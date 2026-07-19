package plexdb

import (
	"fmt"
	"time"
)

// StreamRow is one media_streams row to write.
type StreamRow struct {
	TypeID    int // 1 video, 2 audio, 3 subtitle
	Codec     string
	Language  string
	Index     int
	Channels  int
	Bitrate   int64
	ExtraData string // JSON; ma:profile drives the client's stream label
}

// WriteSpec is the analyzed media info for one part, shaped exactly like
// the rows the official scanner writes (measured against a live Plex).
type WriteSpec struct {
	MediaItemID   int64
	PartID        int64
	Width         int
	Height        int
	Size          int64
	DurationMs    int64
	Bitrate       int64
	Container     string
	VideoCodec    string
	AudioCodec    string
	ColorTRC      string // e.g. smpte2084; empty for SDR
	AspectRatio   float64
	FPS           float64
	AudioChannels int
	Streams       []StreamRow
}

// Backup makes an online snapshot of the database via VACUUM INTO.
func (d *DB) Backup(dst string) error {
	_, err := d.conn.Exec(fmt.Sprintf("VACUUM INTO '%s'", dst))
	return err
}

// UpdateMedia writes the spec inside one transaction: refreshes
// media_items, the part, replaces the streams, and syncs
// metadata_items.duration when it is still unknown. It refuses to run
// against an un whitelisted schema.
func (d *DB) UpdateMedia(spec *WriteSpec) error {
	if ok, v, err := d.SchemaKnown(); err != nil {
		return err
	} else if !ok {
		return fmt.Errorf("schema %q not on the write whitelist", v)
	}

	tx, err := d.conn.Begin()
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback() }()

	now := time.Now().Unix()
	if _, err := tx.Exec(`
		UPDATE media_items SET
			width=?, height=?, size=?, duration=?, bitrate=?,
			container=?, video_codec=?, audio_codec=?, color_trc=?,
			display_aspect_ratio=?, frames_per_second=?, audio_channels=?,
			display_offset=0, media_analysis_version=8, updated_at=?
		WHERE id=?`,
		spec.Width, spec.Height, spec.Size, spec.DurationMs, spec.Bitrate,
		spec.Container, spec.VideoCodec, spec.AudioCodec, spec.ColorTRC,
		spec.AspectRatio, spec.FPS, spec.AudioChannels,
		now, spec.MediaItemID); err != nil {
		return err
	}
	if _, err := tx.Exec(`
		UPDATE media_parts SET size=?, duration=?, updated_at=? WHERE id=?`,
		spec.Size, spec.DurationMs, now, spec.PartID); err != nil {
		return err
	}
	if _, err := tx.Exec(`DELETE FROM media_streams WHERE media_item_id=?`,
		spec.MediaItemID); err != nil {
		return err
	}
	for _, s := range spec.Streams {
		var channels, bitrate any // NULL when unknown
		if s.Channels > 0 {
			channels = s.Channels
		}
		if s.Bitrate > 0 {
			bitrate = s.Bitrate
		}
		var lang any
		if s.Language != "" && s.Language != "und" {
			lang = s.Language
		}
		var extra any
		if s.ExtraData != "" {
			extra = s.ExtraData
		}
		if _, err := tx.Exec(`
			INSERT INTO media_streams
				(stream_type_id, media_item_id, media_part_id, codec, language,
				 "index", channels, bitrate, "default", forced, extra_data, updated_at)
			VALUES (?,?,?,?,?,?,?,?,0,0,?,?)`,
			s.TypeID, spec.MediaItemID, spec.PartID, s.Codec, lang,
			s.Index, channels, bitrate, extra, now); err != nil {
			return err
		}
	}
	if _, err := tx.Exec(`
		UPDATE metadata_items SET duration=?
		WHERE id=(SELECT metadata_item_id FROM media_items WHERE id=?)
		  AND (duration IS NULL OR duration=0)`,
		spec.DurationMs, spec.MediaItemID); err != nil {
		return err
	}
	return tx.Commit()
}

// Checkpoint truncates the WAL after a batch of writes.
func (d *DB) Checkpoint() error {
	_, err := d.conn.Exec("PRAGMA wal_checkpoint(TRUNCATE)")
	return err
}

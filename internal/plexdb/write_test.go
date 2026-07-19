package plexdb

import (
	"path/filepath"
	"strings"
	"testing"
)

// newWriteFixture builds a temp database with write-relevant tables and
// one item carrying a part.
func newWriteFixture(t *testing.T) *DB {
	t.Helper()
	db, err := Open(filepath.Join(t.TempDir(), "lib.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = db.Close() })

	stmts := []string{
		`CREATE TABLE schema_migrations (version varchar(255) NOT NULL)`,
		`INSERT INTO schema_migrations VALUES ('500000000001.231')`,
		`CREATE TABLE metadata_items (id INTEGER PRIMARY KEY, duration INTEGER)`,
		`INSERT INTO metadata_items VALUES (42, NULL)`,
		`CREATE TABLE media_items (id INTEGER PRIMARY KEY, metadata_item_id INTEGER,
		   width INTEGER, height INTEGER, size INTEGER, duration INTEGER, bitrate INTEGER,
		   container TEXT, video_codec TEXT, audio_codec TEXT, color_trc TEXT,
		   display_aspect_ratio REAL, frames_per_second REAL, audio_channels INTEGER,
		   display_offset INTEGER, media_analysis_version INTEGER, updated_at INTEGER)`,
		`INSERT INTO media_items (id, metadata_item_id) VALUES (7, 42)`,
		`CREATE TABLE media_parts (id INTEGER PRIMARY KEY, media_item_id INTEGER,
		   size INTEGER, duration INTEGER, updated_at INTEGER)`,
		`INSERT INTO media_parts (id, media_item_id, size, duration) VALUES (70, 7, 35, NULL)`,
		`CREATE TABLE media_streams (id INTEGER PRIMARY KEY AUTOINCREMENT,
		   stream_type_id INTEGER, media_item_id INTEGER, media_part_id INTEGER,
		   codec TEXT, language TEXT, "index" INTEGER, channels INTEGER, bitrate INTEGER,
		   "default" INTEGER, forced INTEGER, extra_data TEXT, updated_at INTEGER)`,
	}
	for _, s := range stmts {
		if _, err := db.conn.Exec(s); err != nil {
			t.Fatalf("fixture %q: %v", s, err)
		}
	}
	return db
}

func sampleSpec() *WriteSpec {
	return &WriteSpec{
		MediaItemID: 7, PartID: 70,
		Width: 1280, Height: 720, Size: 3757806, DurationMs: 20023,
		Bitrate: 1501395, Container: "mkv", VideoCodec: "h264", AudioCodec: "aac",
		ColorTRC:    "smpte2084",
		AspectRatio: 1.7777777910232544, FPS: 24.0, AudioChannels: 1,
		Streams: []StreamRow{
			{TypeID: 1, Codec: "h264", Index: 0, Bitrate: 1501395,
				ExtraData: `{"ma:profile":"high"}`},
			{TypeID: 2, Codec: "aac", Index: 1, Language: "und", Channels: 2, Bitrate: 96000},
		},
	}
}

func TestUpdateMedia(t *testing.T) {
	db := newWriteFixture(t)

	if err := db.UpdateMedia(sampleSpec()); err != nil {
		t.Fatal(err)
	}

	var w, h, dur int
	var container, trc string
	err := db.conn.QueryRow(
		`SELECT width, height, duration, container, IFNULL(color_trc,'') FROM media_items WHERE id=7`,
	).Scan(&w, &h, &dur, &container, &trc)
	if err != nil || w != 1280 || h != 720 || dur != 20023 || container != "mkv" {
		t.Fatalf("media_items: %v %d %d %d %s", err, w, h, dur, container)
	}
	if trc != "smpte2084" {
		t.Fatalf("color_trc = %q, want smpte2084", trc)
	}

	var pSize, pDur int64
	if err := db.conn.QueryRow(
		`SELECT size, duration FROM media_parts WHERE id=70`).Scan(&pSize, &pDur); err != nil ||
		pSize != 3757806 || pDur != 20023 {
		t.Fatalf("media_parts: %v %d %d", err, pSize, pDur)
	}

	var vCount int
	var vBitrate int64
	var aChannels any
	if err := db.conn.QueryRow(
		`SELECT count(*) FROM media_streams WHERE media_item_id=7 AND stream_type_id=1`).Scan(&vCount); err != nil || vCount != 1 {
		t.Fatalf("video streams: %v %d", err, vCount)
	}
	if err := db.conn.QueryRow(
		`SELECT bitrate FROM media_streams WHERE media_item_id=7 AND stream_type_id=1`).Scan(&vBitrate); err != nil || vBitrate != 1501395 {
		t.Fatalf("video bitrate: %v %d", err, vBitrate)
	}
	if err := db.conn.QueryRow(
		`SELECT channels FROM media_streams WHERE media_item_id=7 AND stream_type_id=2`).Scan(&aChannels); err != nil {
		t.Fatalf("audio channels: %v", err)
	}
	if ch, ok := aChannels.(int64); !ok || ch != 2 {
		t.Fatalf("audio channels = %#v, want 2", aChannels)
	}

	// "und" language must be stored as NULL
	var lang any
	if err := db.conn.QueryRow(
		`SELECT language FROM media_streams WHERE media_item_id=7 AND stream_type_id=2`).Scan(&lang); err != nil || lang != nil {
		t.Fatalf("und language should be NULL: %v %#v", err, lang)
	}

	// profile lands in extra_data
	var extra any
	if err := db.conn.QueryRow(
		`SELECT extra_data FROM media_streams WHERE media_item_id=7 AND stream_type_id=1`).Scan(&extra); err != nil {
		t.Fatal(err)
	}
	if s, ok := extra.(string); !ok || !strings.Contains(s, `"ma:profile":"high"`) {
		t.Fatalf("video extra_data = %#v", extra)
	}

	// metadata duration synced
	var mdur any
	if err := db.conn.QueryRow(
		`SELECT duration FROM metadata_items WHERE id=42`).Scan(&mdur); err != nil {
		t.Fatal(err)
	}
	if d, ok := mdur.(int64); !ok || d != 20023 {
		t.Fatalf("metadata duration = %#v, want 20023", mdur)
	}
}

func TestUpdateMediaReplacesStreams(t *testing.T) {
	db := newWriteFixture(t)
	if err := db.UpdateMedia(sampleSpec()); err != nil {
		t.Fatal(err)
	}
	spec := sampleSpec()
	spec.Streams = spec.Streams[:1] // second write: only video
	if err := db.UpdateMedia(spec); err != nil {
		t.Fatal(err)
	}
	var n int
	if err := db.conn.QueryRow(
		`SELECT count(*) FROM media_streams WHERE media_item_id=7`).Scan(&n); err != nil || n != 1 {
		t.Fatalf("streams after rewrite: %v %d", err, n)
	}
}

func TestUpdateMediaRefusesUnknownSchema(t *testing.T) {
	db := newWriteFixture(t)
	if _, err := db.conn.Exec(`UPDATE schema_migrations SET version='9'`); err != nil {
		t.Fatal(err)
	}
	if err := db.UpdateMedia(sampleSpec()); err == nil {
		t.Fatal("expected refusal on unknown schema")
	}
}

func TestBackupAndCheckpoint(t *testing.T) {
	db := newWriteFixture(t)
	dst := filepath.Join(t.TempDir(), "backup.db")
	if err := db.Backup(dst); err != nil {
		t.Fatal(err)
	}
	bk, err := Open(dst)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = bk.Close() }()
	if _, _, err := bk.SchemaKnown(); err != nil {
		t.Fatalf("backup unreadable: %v", err)
	}
	if err := db.Checkpoint(); err != nil {
		t.Fatal(err)
	}
}

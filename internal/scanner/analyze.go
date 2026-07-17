package scanner

import (
	"context"
	"fmt"
	"strconv"
	"strings"

	"github.com/PlexAssistant/PlexAssistant/internal/linkres"
	"github.com/PlexAssistant/PlexAssistant/internal/plexdb"
)

// Options configures one analysis run.
type Options struct {
	FfprobePath string
	UserAgent   string
}

// AnalyzeItem runs the pipeline for every strm part of one metadata
// item: resolve the direct link, probe it, write the results. Non-strm
// parts and failures are skipped without aborting the run.
func AnalyzeItem(ctx context.Context, db *plexdb.DB, itemID int64, opts Options) (int, error) {
	parts, err := db.PartsByItem(itemID)
	if err != nil {
		return 0, err
	}

	analyzed := 0
	for _, part := range parts {
		if !strings.HasSuffix(strings.ToLower(part.File), ".strm") {
			continue
		}
		target, err := linkres.ParseSTRM(part.File)
		if err != nil {
			fmt.Printf("item %d part %d: %v\n", itemID, part.ID, err)
			continue
		}
		if !target.IsRemote {
			continue // local path inside a strm: not ours
		}

		link, err := linkres.Resolve(ctx, target.Value, opts.UserAgent)
		if err != nil {
			fmt.Printf("item %d part %d: resolve: %v\n", itemID, part.ID, err)
			continue
		}
		probe, err := Probe(ctx, opts.FfprobePath, link.URL, opts.UserAgent)
		if err != nil {
			fmt.Printf("item %d part %d: probe: %v\n", itemID, part.ID, err)
			continue
		}
		if err := db.UpdateMedia(BuildSpec(part, probe, link)); err != nil {
			return analyzed, fmt.Errorf("item %d part %d: write: %w", itemID, part.ID, err)
		}
		analyzed++
	}
	return analyzed, nil
}

// BuildSpec maps a probe result onto the row shapes the official
// scanner writes.
func BuildSpec(part plexdb.Part, probe *ProbeResult, link *linkres.Link) *plexdb.WriteSpec {
	spec := &plexdb.WriteSpec{
		MediaItemID: part.MediaItemID,
		PartID:      part.ID,
		DurationMs:  DurationMs(probe.Format.Duration),
		Bitrate:     atoi64(probe.Format.BitRate),
		Container:   mapContainer(probe.Format.FormatName),
		Size:        atoi64(probe.Format.Size),
	}
	if spec.Size == 0 {
		spec.Size = link.Size
	}

	for _, s := range probe.Streams {
		row := plexdb.StreamRow{
			Codec:    s.CodecName,
			Language: s.Tags.Language,
			Index:    s.Index,
			Channels: s.Channels,
			Bitrate:  atoi64(s.BitRate),
			TypeID:   streamTypeID(s.CodecType),
		}
		// The official rows never store NULL here: und/empty falls back
		// to a language string, and a missing video bitrate uses the
		// format bitrate. Plex's SQL layer rejects NULLs on these.
		if row.Language == "" || row.Language == "und" {
			row.Language = "en"
		}

		switch s.CodecType {
		case "video":
			if row.Bitrate == 0 {
				row.Bitrate = spec.Bitrate
			}
			if spec.VideoCodec == "" {
				spec.VideoCodec = s.CodecName
				spec.Width, spec.Height = s.Width, s.Height
				spec.AspectRatio = parseRatio(s.DisplayAspectRatio)
				if spec.AspectRatio == 0 && s.Height > 0 {
					spec.AspectRatio = float64(s.Width) / float64(s.Height)
				}
				spec.FPS = parseFPS(s.FrameRate)
			}
		case "audio":
			if spec.AudioCodec == "" {
				spec.AudioCodec = s.CodecName
				spec.AudioChannels = s.Channels
			}
		}
		spec.Streams = append(spec.Streams, row)
	}
	return spec
}

func streamTypeID(codecType string) int {
	switch codecType {
	case "video":
		return 1
	case "audio":
		return 2
	case "subtitle":
		return 3
	}
	return 0
}

var containerNames = map[string]string{
	"matroska": "mkv",
	"webm":     "webm",
	"mov":      "mp4",
	"mpegts":   "ts",
	"mpeg":     "mpeg",
	"avi":      "avi",
}

func mapContainer(formatName string) string {
	for _, name := range strings.Split(formatName, ",") {
		if mapped, ok := containerNames[strings.TrimSpace(name)]; ok {
			return mapped
		}
	}
	return strings.TrimSpace(strings.SplitN(formatName, ",", 2)[0])
}

// parseRatio turns "16:9" into 1.777...
func parseRatio(v string) float64 {
	a, b, ok := strings.Cut(v, ":")
	if !ok {
		return 0
	}
	x, err1 := strconv.ParseFloat(a, 64)
	y, err2 := strconv.ParseFloat(b, 64)
	if err1 != nil || err2 != nil || y == 0 {
		return 0
	}
	return x / y
}

// parseFPS turns ffprobe rates like "24/1" or "24000/1001" into float.
func parseFPS(v string) float64 {
	num, den, ok := strings.Cut(v, "/")
	if !ok {
		return atof(v)
	}
	n := atof(num)
	d := atof(den)
	if d == 0 {
		return 0
	}
	return n / d
}

func atof(s string) float64 {
	f, err := strconv.ParseFloat(strings.TrimSpace(s), 64)
	if err != nil {
		return 0
	}
	return f
}

func atoi64(s string) int64 {
	n, err := strconv.ParseInt(strings.TrimSpace(s), 10, 64)
	if err != nil {
		return 0
	}
	return n
}

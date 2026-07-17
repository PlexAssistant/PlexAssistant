package scanner

import (
	"testing"

	"github.com/PlexAssistant/PlexAssistant/internal/linkres"
	"github.com/PlexAssistant/PlexAssistant/internal/plexdb"
)

func TestBuildSpec(t *testing.T) {
	part := plexdb.Part{ID: 70, MediaItemID: 7, File: "/data/a.strm", Size: 35}
	probe, err := parseProbe([]byte(`{
	  "streams": [
	    {"index": 0, "codec_name": "h264", "codec_type": "video",
	     "width": 1280, "height": 720, "bit_rate": "612000",
	     "avg_frame_rate": "24/1", "display_aspect_ratio": "16:9"},
	    {"index": 1, "codec_name": "aac", "codec_type": "audio",
	     "channels": 2, "sample_rate": "48000", "bit_rate": "96000",
	     "tags": {"language": "und"}},
	    {"index": 2, "codec_name": "subrip", "codec_type": "subtitle",
	     "tags": {"language": "chi"}}
	  ],
	  "format": {
	    "format_name": "matroska,webm", "duration": "20.016000",
	    "size": "3757806", "bit_rate": "1501731"
	  }}`))
	if err != nil {
		t.Fatal(err)
	}
	link := &linkres.Link{URL: "http://x/dl"}

	spec := BuildSpec(part, probe, link)
	if spec.MediaItemID != 7 || spec.PartID != 70 {
		t.Fatalf("ids: %+v", spec)
	}
	if spec.DurationMs != 20016 || spec.Size != 3757806 || spec.Bitrate != 1501731 {
		t.Fatalf("format fields: %+v", spec)
	}
	if spec.Container != "mkv" || spec.VideoCodec != "h264" || spec.AudioCodec != "aac" {
		t.Fatalf("codecs: %+v", spec)
	}
	if spec.Width != 1280 || spec.Height != 720 {
		t.Fatalf("dims: %+v", spec)
	}
	// 16:9 parsed, not width/height fallback
	if spec.AspectRatio < 1.7777 || spec.AspectRatio > 1.7778 {
		t.Fatalf("aspect: %v", spec.AspectRatio)
	}
	if spec.FPS != 24 {
		t.Fatalf("fps: %v", spec.FPS)
	}
	if spec.AudioChannels != 2 {
		t.Fatalf("channels: %v", spec.AudioChannels)
	}
	if len(spec.Streams) != 3 {
		t.Fatalf("streams: %d", len(spec.Streams))
	}
	wantTypes := []int{1, 2, 3}
	for i, want := range wantTypes {
		if spec.Streams[i].TypeID != want {
			t.Fatalf("stream[%d] type = %d, want %d", i, spec.Streams[i].TypeID, want)
		}
	}
	if spec.Streams[2].Language != "chi" {
		t.Fatalf("subtitle lang: %+v", spec.Streams[2])
	}
}

func TestBuildSpecFallbacks(t *testing.T) {
	part := plexdb.Part{ID: 1, MediaItemID: 1, File: "/data/a.strm"}
	probe, err := parseProbe([]byte(`{
	  "streams": [
	    {"index": 0, "codec_name": "hevc", "codec_type": "video",
	     "width": 3840, "height": 2160, "avg_frame_rate": "24000/1001"}
	  ],
	  "format": {"format_name": "mpegts", "duration": "10.0"}}`))
	if err != nil {
		t.Fatal(err)
	}
	// no format size/bitrate: fall back to the resolved link size
	spec := BuildSpec(part, probe, &linkres.Link{URL: "x", Size: 12345})
	if spec.Size != 12345 {
		t.Fatalf("size fallback: %d", spec.Size)
	}
	if spec.Bitrate != 0 {
		t.Fatalf("bitrate: %d", spec.Bitrate)
	}
	if spec.Container != "ts" {
		t.Fatalf("container: %s", spec.Container)
	}
	// no display_aspect_ratio: width/height fallback
	if spec.AspectRatio < 1.777 || spec.AspectRatio > 1.778 {
		t.Fatalf("aspect fallback: %v", spec.AspectRatio)
	}
	if got := int(spec.FPS * 1000); got != 23976 {
		t.Fatalf("fps: %v", spec.FPS)
	}
}

func TestParseHelpers(t *testing.T) {
	if r := parseRatio("16:9"); r < 1.777 || r > 1.778 {
		t.Fatalf("parseRatio: %v", r)
	}
	if r := parseRatio("bogus"); r != 0 {
		t.Fatalf("parseRatio bogus: %v", r)
	}
	if f := parseFPS("0/0"); f != 0 {
		t.Fatalf("parseFPS 0/0: %v", f)
	}
	if f := parseFPS("25"); f != 25 {
		t.Fatalf("parseFPS 25: %v", f)
	}
	if c := mapContainer("mov,mp4,m4a,3gp,3g2,mj2"); c != "mp4" {
		t.Fatalf("mapContainer: %s", c)
	}
}

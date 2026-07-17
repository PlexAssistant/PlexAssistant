package scanner

import "testing"

const sampleProbeJSON = `{
  "streams": [
    {
      "index": 0,
      "codec_name": "h264", "codec_type": "video",
      "width": 1280, "height": 720,
      "avg_frame_rate": "24/1", "bit_rate": "612000"
    },
    {
      "index": 1,
      "codec_name": "aac", "codec_type": "audio",
      "channels": 2, "sample_rate": "48000",
      "bit_rate": "96000", "duration": "20.016000",
      "tags": { "language": "und" }
    }
  ],
  "format": {
    "format_name": "matroska,webm",
    "duration": "20.016000",
    "size": "3757806",
    "bit_rate": "1501731"
  }
}`

func TestParseProbeOutput(t *testing.T) {
	res, err := parseProbe([]byte(sampleProbeJSON))
	if err != nil {
		t.Fatal(err)
	}
	if len(res.Streams) != 2 {
		t.Fatalf("streams = %d", len(res.Streams))
	}
	v := res.Streams[0]
	if v.CodecType != "video" || v.Width != 1280 || v.Height != 720 {
		t.Fatalf("video stream = %+v", v)
	}
	a := res.Streams[1]
	if a.CodecType != "audio" || a.Channels != 2 || a.Tags.Language != "und" {
		t.Fatalf("audio stream = %+v", a)
	}
	if got := DurationMs(res.Format.Duration); got != 20016 {
		t.Fatalf("duration = %d ms", got)
	}
}

func TestDurationMs(t *testing.T) {
	tests := []struct {
		in   string
		want int64
	}{
		{"20.016000", 20016},
		{"0.5", 500},
		{"", 0},
		{"N/A", 0},
		{"junk", 0},
	}
	for _, tt := range tests {
		if got := DurationMs(tt.in); got != tt.want {
			t.Errorf("DurationMs(%q) = %d, want %d", tt.in, got, tt.want)
		}
	}
}

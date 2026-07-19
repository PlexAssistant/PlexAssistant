package scanner

import (
	"fmt"
	"net/url"
	"strings"
)

// buildVideoExtra composes the stream extra_data the way the official
// analysis does: a JSON object of ma: fields plus a url-encoded copy.
// PMS derives the client's displayTitle ("2160p HDR10", ...) from them.
func buildVideoExtra(s *Stream) string {
	fields := orderedFields{
		{"ma:bitDepth", videoBitDepth(s)},
		{"ma:chromaLocation", s.ChromaLocation},
		{"ma:chromaSubsampling", chromaSubsampling(s.PixFmt)},
		{"ma:codedHeight", itoaOr(s.CodedHeight, s.Height)},
		{"ma:codedWidth", itoaOr(s.CodedWidth, s.Width)},
		{"ma:colorPrimaries", s.ColorPrimaries},
		{"ma:colorRange", s.ColorRange},
		{"ma:colorSpace", s.ColorSpace},
		{"ma:colorTrc", s.ColorTransfer},
		{"ma:frameRate", formatFrameRate(s.FrameRate)},
		{"ma:hasScalingMatrix", "0"},
		{"ma:height", itoaOr(s.Height, 0)},
		{"ma:level", itoaOr(s.Level, 0)},
		{"ma:profile", strings.ToLower(s.Profile)},
		{"ma:refFrames", itoaOr(s.Refs, 0)},
		{"ma:scanType", "progressive"},
		{"ma:width", itoaOr(s.Width, 0)},
	}

	var jsonBuf strings.Builder
	query := url.Values{}
	jsonBuf.WriteByte('{')
	first := true
	for _, f := range fields {
		if f.value == "" {
			continue
		}
		if !first {
			jsonBuf.WriteByte(',')
		}
		first = false
		fmt.Fprintf(&jsonBuf, "%q:%q", f.key, f.value)
		query.Set(f.key, f.value)
	}
	fmt.Fprintf(&jsonBuf, ",\"url\":%q}", query.Encode())
	return jsonBuf.String()
}

type orderedFields []struct{ key, value string }

func itoaOr(v, fallback int) string {
	if v == 0 {
		v = fallback
	}
	if v == 0 {
		return ""
	}
	return fmt.Sprintf("%d", v)
}

// videoBitDepth prefers bits_per_raw_sample, then the pix_fmt suffix.
func videoBitDepth(s *Stream) string {
	if s.BitDepth != "" && s.BitDepth != "N/A" {
		return s.BitDepth
	}
	for _, suffix := range []string{"16le", "12le", "10le", "16", "12", "10"} {
		if strings.HasSuffix(s.PixFmt, suffix) {
			return strings.TrimSuffix(suffix, "le")
		}
	}
	if s.PixFmt != "" {
		return "8"
	}
	return ""
}

// chromaSubsampling maps pix_fmt names to the sampling notation.
func chromaSubsampling(pixFmt string) string {
	fmt := strings.TrimSuffix(strings.TrimSuffix(pixFmt, "le"), "be")
	fmt = strings.TrimRight(fmt, "0123456789")
	switch fmt {
	case "yuv420p":
		return "4:2:0"
	case "yuv422p":
		return "4:2:2"
	case "yuv444p":
		return "4:4:4"
	}
	return ""
}

// formatFrameRate renders "24/1" as "24.000".
func formatFrameRate(v string) string {
	fps := parseFPS(v)
	if fps == 0 {
		return ""
	}
	return fmt.Sprintf("%.3f", fps)
}

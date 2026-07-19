package scanner

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strconv"
	"strings"
	"time"
)

// probeArgs mirrors the measured parameter line: bounded probe size so a
// stray huge file cannot stall analysis, and an explicit protocol
// whitelist for remote inputs.
func probeArgs(rawURL, ua string) []string {
	return []string{
		"-v", "error",
		"-probesize", "8388608",
		"-protocol_whitelist", "file,http,https,tcp,tls",
		"-user_agent", ua,
		"-show_format", "-show_streams",
		"-of", "json",
		rawURL,
	}
}

// Probe runs ffprobe against rawURL and parses the result.
func Probe(ctx context.Context, ffprobePath, rawURL, ua string) (*ProbeResult, error) {
	ctx, cancel := context.WithTimeout(ctx, 2*time.Minute)
	defer cancel()

	out, err := exec.CommandContext(ctx, ffprobePath, probeArgs(rawURL, ua)...).Output()
	if err != nil {
		if ee, ok := err.(*exec.ExitError); ok && len(ee.Stderr) > 0 {
			return nil, fmt.Errorf("ffprobe: %s", strings.TrimSpace(string(ee.Stderr)))
		}
		return nil, fmt.Errorf("ffprobe: %w", err)
	}
	return parseProbe(out)
}

func parseProbe(out []byte) (*ProbeResult, error) {
	res := &ProbeResult{}
	if err := json.Unmarshal(out, res); err != nil {
		return nil, fmt.Errorf("ffprobe output: %w", err)
	}
	return res, nil
}

// ProbeResult is the subset of ffprobe's JSON output we consume.
type ProbeResult struct {
	Format  Format   `json:"format"`
	Streams []Stream `json:"streams"`
}

// Format is ffprobe's format section.
type Format struct {
	FormatName string `json:"format_name"`
	Duration   string `json:"duration"` // seconds, e.g. "20.016000"
	BitRate    string `json:"bit_rate"`
	Size       string `json:"size"`
}

// Stream is one ffprobe stream entry.
type Stream struct {
	Index              int    `json:"index"`
	CodecName          string `json:"codec_name"`
	Profile            string `json:"profile"`    // e.g. "Main 10", "High"
	CodecType          string `json:"codec_type"` // video | audio | subtitle
	Width              int    `json:"width"`
	Height             int    `json:"height"`
	BitRate            string `json:"bit_rate"`
	Duration           string `json:"duration"`
	Channels           int    `json:"channels"`
	SampleRate         string `json:"sample_rate"`
	FrameRate          string `json:"avg_frame_rate"`
	ColorTransfer      string `json:"color_transfer"`       // e.g. "smpte2084" (HDR10)
	DisplayAspectRatio string `json:"display_aspect_ratio"` // e.g. "16:9"
	Tags               struct {
		Language string `json:"language"`
	} `json:"tags"`
}

// DurationMs parses ffprobe's seconds string into milliseconds.
func DurationMs(s string) int64 {
	if s == "" || s == "N/A" {
		return 0
	}
	f, err := strconv.ParseFloat(s, 64)
	if err != nil {
		return 0
	}
	return int64(f * 1000)
}

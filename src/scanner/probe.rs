use std::io;
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;

const PROBE_TIMEOUT: Duration = Duration::from_secs(2 * 60);

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("ffprobe: {0}")]
    Exec(#[from] io::Error),
    #[error("ffprobe: {0}")]
    Failed(String),
    #[error("ffprobe: timed out after {}s", PROBE_TIMEOUT.as_secs())]
    Timeout,
    #[error("ffprobe output: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Mirrors the measured parameter line: bounded probe size so a stray huge
/// file cannot stall analysis, and an explicit protocol whitelist for
/// remote inputs.
fn probe_args<'a>(url: &'a str, ua: &'a str) -> [&'a str; 13] {
    [
        "-v",
        "error",
        "-probesize",
        "8388608",
        "-protocol_whitelist",
        "file,http,https,tcp,tls",
        "-user_agent",
        ua,
        "-show_format",
        "-show_streams",
        "-of",
        "json",
        url,
    ]
}

/// Runs ffprobe against url and parses the result.
pub async fn probe(ffprobe: &str, url: &str, ua: &str) -> Result<ProbeResult, ProbeError> {
    let child = Command::new(ffprobe)
        .args(probe_args(url, ua))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true) // a timed-out probe must not linger
        .spawn()?;
    let out = tokio::time::timeout(PROBE_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| ProbeError::Timeout)??;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stderr = stderr.trim();
        return Err(ProbeError::Failed(if stderr.is_empty() {
            out.status.to_string()
        } else {
            stderr.to_owned()
        }));
    }
    parse_probe(&out.stdout)
}

pub(super) fn parse_probe(out: &[u8]) -> Result<ProbeResult, ProbeError> {
    Ok(serde_json::from_slice(out)?)
}

/// The subset of ffprobe's JSON output we consume. Missing fields default
/// to zero values, as with Go's decoder.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ProbeResult {
    pub format: Format,
    pub streams: Vec<Stream>,
}

/// ffprobe's format section.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Format {
    pub format_name: String,
    /// seconds, e.g. "20.016000"
    pub duration: String,
    pub bit_rate: String,
    pub size: String,
}

/// One ffprobe stream entry.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Stream {
    pub index: i64,
    pub codec_name: String,
    /// e.g. "Main 10", "High"
    pub profile: String,
    /// video | audio | subtitle
    pub codec_type: String,
    pub width: i64,
    pub height: i64,
    pub coded_width: i64,
    pub coded_height: i64,
    pub bit_rate: String,
    pub duration: String,
    pub channels: i64,
    pub sample_rate: String,
    #[serde(rename = "avg_frame_rate")]
    pub frame_rate: String,
    /// e.g. "smpte2084" (HDR10)
    pub color_transfer: String,
    /// e.g. "bt2020"
    pub color_primaries: String,
    /// e.g. "bt2020nc"
    pub color_space: String,
    /// "tv" | "pc"
    pub color_range: String,
    /// e.g. "yuv420p10le"
    pub pix_fmt: String,
    pub chroma_location: String,
    #[serde(rename = "bits_per_raw_sample")]
    pub bit_depth: String,
    pub level: i64,
    pub refs: i64,
    /// e.g. "16:9"
    pub display_aspect_ratio: String,
    pub tags: Tags,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Tags {
    pub language: String,
}

/// Parses ffprobe's seconds string into milliseconds, rounded like Plex
/// ("4.023000" is 4022.999.. as a float); 0 when unknown.
pub fn duration_ms(s: &str) -> i64 {
    if s.is_empty() || s == "N/A" {
        return 0;
    }
    s.parse::<f64>()
        .map_or(0, |secs| (secs * 1000.0).round() as i64)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    pub(in crate::scanner) const SAMPLE_PROBE_JSON: &str = r#"{
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
    }"#;

    #[test]
    fn parse_output() {
        let res = parse_probe(SAMPLE_PROBE_JSON.as_bytes()).unwrap();
        assert_eq!(res.streams.len(), 2);
        let v = &res.streams[0];
        assert_eq!(
            (v.codec_type.as_str(), v.width, v.height),
            ("video", 1280, 720)
        );
        assert_eq!(v.frame_rate, "24/1");
        let a = &res.streams[1];
        assert_eq!((a.codec_type.as_str(), a.channels), ("audio", 2));
        assert_eq!(a.tags.language, "und");
        assert_eq!(duration_ms(&res.format.duration), 20016);
    }

    #[test]
    fn parse_tolerates_extra_and_missing_fields() {
        let res = parse_probe(br#"{"streams":[{"codec_type":"video","unknown":1}]}"#).unwrap();
        assert_eq!(res.streams[0].codec_type, "video");
        assert_eq!(res.format, Format::default());
        assert!(parse_probe(b"not json").is_err());
    }

    #[test]
    fn durations() {
        for (input, want) in [
            ("20.016000", 20016),
            ("4.023000", 4023),
            ("4.014667", 4015),
            ("0.5", 500),
            ("", 0),
            ("N/A", 0),
            ("junk", 0),
        ] {
            assert_eq!(duration_ms(input), want, "{input:?}");
        }
    }

    /// Writes an executable stand-in for ffprobe.
    pub(in crate::scanner) fn fake_ffprobe(dir: &tempfile::TempDir, script: &str) -> String {
        let path = dir.path().join("ffprobe");
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn runs_ffprobe_with_measured_args() {
        let dir = tempfile::tempdir().unwrap();
        let args_file = dir.path().join("args");
        let ffprobe = fake_ffprobe(
            &dir,
            &format!(
                "printf '%s\\n' \"$@\" > '{}'\ncat <<'JSON'\n{SAMPLE_PROBE_JSON}\nJSON",
                args_file.display()
            ),
        );
        let res = probe(&ffprobe, "http://x/dl", "UA/1").await.unwrap();
        assert_eq!(res.streams.len(), 2);
        let args = std::fs::read_to_string(args_file).unwrap();
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            probe_args("http://x/dl", "UA/1")
        );
    }

    #[tokio::test]
    async fn failure_reports_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let ffprobe = fake_ffprobe(&dir, "echo 'Server returned 404 Not Found' >&2\nexit 1");
        let err = probe(&ffprobe, "http://x/dl", "UA").await.unwrap_err();
        assert_eq!(err.to_string(), "ffprobe: Server returned 404 Not Found");

        let silent = fake_ffprobe(&dir, "exit 3");
        let err = probe(&silent, "http://x/dl", "UA").await.unwrap_err();
        assert_eq!(err.to_string(), "ffprobe: exit status: 3");
    }

    #[tokio::test]
    async fn missing_binary() {
        let err = probe("/nonexistent/ffprobe", "http://x", "UA")
            .await
            .unwrap_err();
        assert!(matches!(err, ProbeError::Exec(_)), "{err:?}");
    }
}

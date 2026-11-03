//! Black-window credits detection: ffmpeg's blackdetect over the tail.

use tokio::process::Command;

use super::TAIL_SECS;
use crate::scanner::redact_urls;

/// A black interval ffmpeg's blackdetect reported, in seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlackInterval {
    pub start: f64,
    pub end: f64,
}

/// Minimum marker window Plex honors.
const MIN_MARKER_SECS: f64 = 15.0;

/// Heuristic credits detection: run ffmpeg's blackdetect over the last
/// ten minutes and pick the last black window of at least 15 seconds as
/// the credits. Misses credits that are not black-background — the
/// visual detectors are the follow-up for those. `duration_secs` places
/// the seek; 0 scans from the start.
pub async fn detect(ffmpeg: &str, input: &str, ua: &str, duration_secs: f64) -> Option<(i64, i64)> {
    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-nostats".into(),
        "-loglevel".into(),
        "info".into(),
    ];
    if input.starts_with("http://") || input.starts_with("https://") {
        // ffmpeg (unlike ffprobe) has no -user_agent option; the header
        // form carries the same UA to the CDN
        args.push("-headers".into());
        args.push(format!("User-Agent: {ua}\r\n"));
    }
    // seek into the tail; blackdetect timestamps are relative to it
    let seek = (duration_secs - TAIL_SECS).max(0.0);
    if seek > 0.0 {
        args.push("-ss".into());
        args.push(format!("{seek:.3}"));
    }
    args.extend([
        "-i".to_owned(),
        input.to_owned(),
        "-an".into(),
        "-vf".into(),
        // 0.5 fps sampling + 320px, matching the
        // extraction; pic_th tolerates credit text and stray subtitles
        "fps=0.5,scale=w=320:h=-2,blackdetect=d=2:pix_th=0.10:pic_th=0.90".into(),
        "-f".into(),
        "null".into(),
        "-".into(),
    ]);
    let out = Command::new(ffmpeg)
        .args(&args)
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    // blackdetect reports on stderr
    let log = String::from_utf8_lossy(&out.stderr);
    // lines are chronological; the last long enough window wins
    let Some(interval) = log
        .lines()
        .rev()
        .filter_map(parse_blackdetect)
        .find(|i| i.end - i.start >= MIN_MARKER_SECS)
    else {
        // the info-level log echoes the signed input URL; redact it
        let tail: Vec<String> = log.lines().rev().take(3).map(redact_urls).collect();
        eprintln!(
            "credits: no window; ffmpeg exit={:?}, stderr tail: {:?}",
            out.status.code(),
            tail
        );
        return None;
    };
    Some((
        ((interval.start + seek) * 1000.0) as i64,
        ((interval.end + seek) * 1000.0) as i64,
    ))
}

/// Parses a blackdetect log line:
/// `[blackdetect @ 0x...] black_start: 30.016 black_end: 59.983 black_duration: 29.967`
pub fn parse_blackdetect(line: &str) -> Option<BlackInterval> {
    if !line.contains("black_start:") {
        return None;
    }
    let field = |name: &str| -> Option<f64> {
        let idx = line.find(name)? + name.len();
        let rest = line[idx..].trim_start();
        let end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        rest[..end].parse().ok()
    };
    Some(BlackInterval {
        start: field("black_start:")?,
        end: field("black_end:")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blackdetect_lines() {
        let line =
            "[blackdetect @ 0xaaa] black_start: 30.016 black_end: 59.983 black_duration: 29.967";
        assert_eq!(
            parse_blackdetect(line),
            Some(BlackInterval {
                start: 30.016,
                end: 59.983
            })
        );
        assert_eq!(parse_blackdetect("[out#0/null @ 0x1] video:0kB"), None);
        // truncated line without an end yields nothing
        assert_eq!(parse_blackdetect("black_start: 10"), None);
    }

    #[tokio::test]
    async fn detects_on_a_real_file() {
        // build 40s: 10s testsrc + 30s black with a scrolling credit line
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("credits.mkv");
        let font = "/System/Library/Fonts/Supplemental/Arial.ttf";
        if !std::path::Path::new(font).exists() {
            return; // non-mac runner: skip, the parser tests carry the logic
        }
        let st = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x240:rate=10:duration=10",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=320x240:r=10:d=30",
                "-filter_complex",
                &format!(
                    "[1:v]drawtext=text=CREDITS:fontfile={font}:\
                     fontcolor=white:fontsize=16:x=(w-tw)/2:y=h-mod(t*40\\,h+th)[t];\
                     [0:v][t]concat=n=2:v=1:a=0[v]"
                ),
                "-map",
                "[v]",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-crf",
                "35",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&file)
            .status()
            .unwrap();
        assert!(st.success());
        let marker = detect("ffmpeg", file.to_str().unwrap(), "UA/1", 40.0).await;
        let (start, end) = marker.expect("black credits window found");
        assert!((9_000..=12_000).contains(&start), "start {start}");
        assert!(end >= 38_000, "end {end}");
    }
}

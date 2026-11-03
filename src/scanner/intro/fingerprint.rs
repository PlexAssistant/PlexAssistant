//! Chromaprint fingerprints of an episode's head.

use std::path::Path;

use tokio::process::Command;

/// Seconds of audio to fingerprint from the head of each episode.
/// Most intros live within the first two minutes.
pub const HEAD_SECS: i64 = 120;

/// Extracts a chromaprint fingerprint from the head of a media file.
/// ffmpeg decodes the audio (with the bound user agent for cloud
/// links) into a WAV tempfile; fpcalc reads it and prints raw
/// fingerprint integers.
pub async fn extract_fingerprint(
    ffmpeg: &str,
    fpcalc: &str,
    input: &str,
    ua: &str,
    tmpdir: &Path,
) -> Option<Vec<u32>> {
    let wav = tmpdir.join("fp_audio.wav");

    // decode the audio head with our user agent
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
    ];
    if input.starts_with("http://") || input.starts_with("https://") {
        args.push("-headers".into());
        args.push(format!("User-Agent: {ua}\r\n"));
    }
    args.extend([
        "-i".into(),
        input.to_owned(),
        "-t".into(),
        HEAD_SECS.to_string(),
        "-vn".into(),
        "-ac".into(),
        "1".into(),
        "-ar".into(),
        "11025".into(),
        "-c:a".into(),
        "pcm_s16le".into(),
        "-f".into(),
        "wav".into(),
        wav.to_string_lossy().into_owned(),
    ]);
    let st = Command::new(ffmpeg)
        .args(&args)
        .kill_on_drop(true)
        .status()
        .await
        .ok()?;
    if !st.success() {
        eprintln!("intro: ffmpeg audio extract failed: {st}");
        return None;
    }

    // fingerprint the WAV
    let out = Command::new(fpcalc)
        .args(["-raw", "-length", &HEAD_SECS.to_string()])
        .arg(&wav)
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let _ = tokio::fs::remove_file(&wav).await;
    parse_fingerprint(&text)
}

/// Parses fpcalc's `FINGERPRINT=a,b,c,...` output line.
pub fn parse_fingerprint(text: &str) -> Option<Vec<u32>> {
    let line = text.lines().find(|l| l.starts_with("FINGERPRINT="))?;
    let raw = line.trim_start_matches("FINGERPRINT=");
    let values: Vec<u32> = raw
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    (values.len() > 10).then_some(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fpcalc_output() {
        let text = "DURATION=120\nFINGERPRINT=1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16\n";
        let fp = parse_fingerprint(text).unwrap();
        assert_eq!(fp.len(), 16);
        assert_eq!(fp[0], 1);
        assert_eq!(fp[15], 16);
        assert!(parse_fingerprint("garbage").is_none());
        assert!(parse_fingerprint("FINGERPRINT=1,2,3").is_none()); // too short
    }
}

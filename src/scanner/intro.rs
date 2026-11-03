use std::collections::HashMap;
use std::path::PathBuf;

use tokio::process::Command;

use crate::linkres;
use crate::plexdb::{Db, MARKER_INTRO};
use crate::runtime::Client;

/// Seconds of audio to fingerprint from the head of each episode.
/// Most intros live within the first two minutes.
pub const HEAD_SECS: i64 = 120;

/// Chromaprint produces one 32-bit entry per ~0.1238 seconds at the
/// default settings; the exact step depends on the sample rate.
const FINGERPRINT_STEP_SECS: f64 = 0.1238;

/// Minimum intro duration for a marker (Plex honors ≥15s).
const MIN_INTRO_SECS: f64 = 15.0;

/// Bit similarity threshold: two entries match when enough of their
/// 32 bits agree. 0.75 works well for chromaprint's perceptual hashing.
const SIMILARITY_THRESHOLD: f64 = 0.75;

/// Extracts a chromaprint fingerprint from the head of a media file.
/// ffmpeg decodes the audio (with the bound user agent for cloud
/// links) into a WAV tempfile; fpcalc reads it and prints raw
/// fingerprint integers.
pub async fn extract_fingerprint(
    ffmpeg: &str,
    fpcalc: &str,
    input: &str,
    ua: &str,
    tmpdir: &std::path::Path,
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

/// One matching segment between two fingerprints.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Match {
    /// start in seconds (relative to the fingerprint's head)
    pub start: f64,
    /// end in seconds
    pub end: f64,
    /// average bit similarity across the segment
    pub score: f64,
}

/// Bit similarity between two chromaprint entries: the fraction of the
/// 32 bits that agree.
fn similarity(a: u32, b: u32) -> f64 {
    let matching_bits = 32 - (a ^ b).count_ones();
    matching_bits as f64 / 32.0
}

/// Finds the longest stretch of high-similarity entries between two
/// fingerprints at the same offset (both are aligned from the start of
/// their files; intros start near the beginning in both).
pub fn find_common_intro(a: &[u32], b: &[u32]) -> Option<Match> {
    let len = a.len().min(b.len());
    if len < 10 {
        return None;
    }

    // score each position
    let scores: Vec<f64> = (0..len).map(|i| similarity(a[i], b[i])).collect();

    // find the longest run of consecutive positions above threshold
    let mut best: Option<Match> = None;
    let mut run_start = 0;
    let mut run_scores: Vec<f64> = Vec::new();

    for (i, &score) in scores
        .iter()
        .enumerate()
        .chain(std::iter::once((len, &0.0)))
    {
        let above = i < len && score >= SIMILARITY_THRESHOLD;
        if above {
            if run_scores.is_empty() {
                run_start = i;
            }
            run_scores.push(scores[i]);
        } else if !run_scores.is_empty() {
            let start = run_start as f64 * FINGERPRINT_STEP_SECS;
            let end = i as f64 * FINGERPRINT_STEP_SECS;
            let duration = end - start;
            let avg: f64 = run_scores.iter().sum::<f64>() / run_scores.len() as f64;
            if duration >= MIN_INTRO_SECS {
                let is_better = best.map(|b| duration > b.end - b.start).unwrap_or(true);
                if is_better {
                    best = Some(Match {
                        start,
                        end,
                        score: avg,
                    });
                }
            }
            run_scores.clear();
        }
    }
    best
}

/// Detects intros for every season in a library section: fingerprint
/// each strm episode's head, cross-compare within the season, and write
/// intro markers for the consensus segment. Returns the number of
/// markers written.
pub async fn detect_section_intros(
    db: &mut Db,
    section_id: i64,
    ffmpeg: &str,
    fpcalc: &str,
    _ffprobe: &str,
    ua: &str,
    sock: Option<&PathBuf>,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let seasons = db.seasons_by_section(section_id)?;
    let tmpdir = tempfile::tempdir().map_err(|e| format!("tmpdir: {e}"))?;
    let mut written = 0;

    for season_id in seasons {
        let episodes = db.strm_episodes_by_season(season_id)?;
        if episodes.len() < 2 {
            continue; // need at least two episodes to compare
        }
        println!("season {season_id}: {} strm episode(s)", episodes.len());

        // fingerprint each episode's head
        let mut fingerprints: Vec<(i64, Vec<u32>)> = Vec::new();
        for (item_id, part) in &episodes {
            let link = match resolve(part, ua, sock).await {
                Some(url) => url,
                None => {
                    println!("  item {item_id}: resolve failed, skipping");
                    continue;
                }
            };
            match extract_fingerprint(ffmpeg, fpcalc, &link, ua, tmpdir.path()).await {
                Some(fp) => {
                    println!("  item {item_id}: {} fingerprint entries", fp.len());
                    fingerprints.push((*item_id, fp));
                }
                None => println!("  item {item_id}: fingerprint failed"),
            }
        }
        if fingerprints.len() < 2 {
            continue;
        }

        // compare all pairs, collect per-item matches
        let mut matches: HashMap<i64, Vec<Match>> = HashMap::new();
        for i in 0..fingerprints.len() {
            for j in (i + 1)..fingerprints.len() {
                let (id_a, fp_a) = &fingerprints[i];
                let (id_b, fp_b) = &fingerprints[j];
                if let Some(m) = find_common_intro(fp_a, fp_b) {
                    matches.entry(*id_a).or_default().push(m);
                    matches.entry(*id_b).or_default().push(m);
                }
            }
        }

        // write the consensus marker (median of match endpoints)
        for (item_id, ms) in &matches {
            if ms.is_empty() {
                continue;
            }
            let mut starts: Vec<f64> = ms.iter().map(|m| m.start).collect();
            let mut ends: Vec<f64> = ms.iter().map(|m| m.end).collect();
            starts.sort_by(|a, b| a.partial_cmp(b).unwrap());
            ends.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let start = starts[starts.len() / 2];
            let end = ends[ends.len() / 2];

            db.write_marker(
                *item_id,
                MARKER_INTRO,
                (start * 1000.0) as i64,
                (end * 1000.0) as i64,
            )?;
            written += 1;
            println!(
                "  item {item_id}: intro {start:.1}-{end:.1}s ({} pairs)",
                ms.len()
            );
        }
    }
    Ok(written)
}

/// Resolves a strm part to a direct link through the daemon or directly.
async fn resolve(part: &crate::plexdb::Part, ua: &str, sock: Option<&PathBuf>) -> Option<String> {
    let target = linkres::parse_strm(&part.file).ok()?;
    if !target.is_remote {
        return None;
    }
    if let Some(sock) = sock
        && let Ok(client) = Client::new(sock)
        && let Ok(res) = client.resolve(&target.value).await
    {
        return Some(res.direct);
    }
    linkres::resolve(&target.value, ua)
        .await
        .ok()
        .map(|l| l.url)
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

    #[test]
    fn finds_identical_intro() {
        // 200 entries ≈ 24.76 seconds; first 150 are identical, last 50 differ
        let common: Vec<u32> = (0..150).map(|i| 0xDEAD_BEEF ^ i).collect();
        let divergent_a: Vec<u32> = (0..50).map(|i| 0x1234_5678u32.wrapping_add(i)).collect();
        let divergent_b: Vec<u32> = (0..50).map(|i| 0x9ABC_DEF0u32.wrapping_sub(i)).collect();

        let a: Vec<u32> = common.iter().chain(divergent_a.iter()).cloned().collect();
        let b: Vec<u32> = common.iter().chain(divergent_b.iter()).cloned().collect();

        let m = find_common_intro(&a, &b).expect("identical head must match");
        assert!(m.start < 1.0, "starts near zero: {}", m.start);
        assert!(m.end > 15.0, "at least 15 seconds: {}", m.end);
        assert!(m.score > 0.95, "near-perfect similarity: {}", m.score);
    }

    #[test]
    fn no_match_when_different() {
        let a: Vec<u32> = (0..200).map(|i| i as u32 * 3).collect();
        let b: Vec<u32> = (0..200).map(|i| i as u32 * 7 + 1).collect();
        assert!(find_common_intro(&a, &b).is_none());
    }

    #[test]
    fn too_short_is_rejected() {
        // only 50 common entries ≈ 6.19 seconds, below the 15s minimum
        let common: Vec<u32> = (0..50).map(|i| 0xCAFE_F00D ^ i).collect();
        let rest_a: Vec<u32> = (0..100).map(|i| i as u32).collect();
        let rest_b: Vec<u32> = (0..100).map(|i| (i * 13 + 7) as u32).collect();
        let a: Vec<u32> = common.iter().chain(rest_a.iter()).cloned().collect();
        let b: Vec<u32> = common.iter().chain(rest_b.iter()).cloned().collect();
        assert!(find_common_intro(&a, &b).is_none());
    }
}

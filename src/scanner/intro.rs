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

/// One matching segment between two fingerprints, with the offset
/// preserved so each episode gets its own intro times.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Match {
    /// offset in entries: B's match starts this many entries after A's
    pub offset: i32,
    /// match start in A's timeline (seconds)
    pub start_a: f64,
    /// match end in A's timeline
    pub end_a: f64,
    /// match start in B's timeline
    pub start_b: f64,
    /// match end in B's timeline
    pub end_b: f64,
    /// average bit similarity across the segment
    pub score: f64,
}

impl Match {
    pub fn duration(&self) -> f64 {
        self.end_a - self.start_a
    }
}

/// Bit similarity between two chromaprint entries: the fraction of the
/// 32 bits that agree.
fn similarity(a: u32, b: u32) -> f64 {
    let matching_bits = 32 - (a ^ b).count_ones();
    matching_bits as f64 / 32.0
}

/// The longest matching run between two score slices.
fn longest_run(scores: &[f64]) -> Option<(usize, usize, f64)> {
    let mut best: Option<(usize, usize, f64)> = None;
    let mut run_start = 0;
    let mut run_sum = 0.0;
    let mut run_len = 0usize;

    let better = |best: &Option<(usize, usize, f64)>, len: usize| {
        best.is_none_or(|(bs, be, _)| len > be - bs)
    };

    for (i, &s) in scores.iter().enumerate() {
        if s >= SIMILARITY_THRESHOLD {
            if run_len == 0 {
                run_start = i;
            }
            run_sum += s;
            run_len += 1;
        } else if run_len > 0 {
            let avg = run_sum / run_len as f64;
            if better(&best, run_len) {
                best = Some((run_start, i, avg));
            }
            run_sum = 0.0;
            run_len = 0;
        }
    }
    // trailing run
    if run_len > 0 {
        let avg = run_sum / run_len as f64;
        if better(&best, run_len) {
            best = Some((run_start, scores.len(), avg));
        }
    }
    best
}

/// Finds the best-aligned matching segment between two fingerprints by
/// sliding one over the other. Each episode gets its own intro times
/// (offset by the alignment), so shows whose intros start at different
/// positions per episode are handled.
///
/// The search covers offsets within ±SEARCH_ENTRIES (≈60 seconds at
/// the default step). For 120-second fingerprints this is ~970 × 970
/// comparisons — about a million popcounts, well under a millisecond.
const SEARCH_ENTRIES: i32 = 484; // ≈60 seconds

pub fn find_common_intro(a: &[u32], b: &[u32]) -> Option<Match> {
    if a.len() < 10 || b.len() < 10 {
        return None;
    }

    let mut best: Option<Match> = None;

    // try offset 0 first (the common case: intro at the head in both)
    if let Some(m) = try_offset(a, b, 0) {
        best = Some(m);
    }
    // if offset 0 already found a long match, we're done
    if let Some(ref m) = best
        && m.duration() >= MIN_INTRO_SECS
    {
        return best;
    }

    // otherwise scan all plausible offsets (skip 0, already tried)
    for offset in -SEARCH_ENTRIES..=SEARCH_ENTRIES {
        if offset == 0 {
            continue;
        }
        if let Some(m) = try_offset(a, b, offset)
            && m.duration() > best.as_ref().map(|b| b.duration()).unwrap_or(0.0)
        {
            best = Some(m);
        }
    }

    // only report matches that meet the minimum duration
    best.filter(|m| m.duration() >= MIN_INTRO_SECS)
}

/// Scores one alignment offset and builds a Match if a long enough
/// run exists.
fn try_offset(a: &[u32], b: &[u32], offset: i32) -> Option<Match> {
    let (a_lo, a_hi, b_lo) = if offset >= 0 {
        let b_lo = offset as usize;
        if b_lo >= b.len() {
            return None;
        }
        let a_hi = (b.len() - b_lo).min(a.len());
        (0, a_hi, b_lo)
    } else {
        let a_lo = (-offset) as usize;
        if a_lo >= a.len() {
            return None;
        }
        let a_hi = a.len().min(a_lo + b.len());
        (a_lo, a_hi, 0)
    };
    let overlap = a_hi.saturating_sub(a_lo);
    if overlap < 10 {
        return None;
    }

    let scores: Vec<f64> = (0..overlap)
        .map(|i| similarity(a[a_lo + i], b[b_lo + i]))
        .collect();
    let (run_start, run_end, avg) = longest_run(&scores)?;

    let start_a = (a_lo + run_start) as f64 * FINGERPRINT_STEP_SECS;
    let end_a = (a_lo + run_end) as f64 * FINGERPRINT_STEP_SECS;
    let start_b = (b_lo + run_start) as f64 * FINGERPRINT_STEP_SECS;
    let end_b = (b_lo + run_end) as f64 * FINGERPRINT_STEP_SECS;

    Some(Match {
        offset,
        start_a,
        end_a,
        start_b,
        end_b,
        score: avg,
    })
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
    let mut written = 0;
    for season_id in seasons {
        written += detect_season_intros(db, season_id, ffmpeg, fpcalc, ua, sock).await?;
    }
    Ok(written)
}

/// Detects intros for one season. A season with fewer than two strm
/// episodes writes nothing. Returns the number of markers written.
pub async fn detect_season_intros(
    db: &mut Db,
    season_id: i64,
    ffmpeg: &str,
    fpcalc: &str,
    ua: &str,
    sock: Option<&PathBuf>,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let tmpdir = tempfile::tempdir().map_err(|e| format!("tmpdir: {e}"))?;
    let mut written = 0;

    {
        let episodes = db.strm_episodes_by_season(season_id)?;
        if episodes.len() < 2 {
            return Ok(0); // need at least two episodes to compare
        }
        println!("season {season_id}: {} strm episode(s)", episodes.len());

        // fingerprint each episode's head. An episode whose content has
        // not changed, or whose marker was set by hand, is not fingerprinted
        // again; the season still needs two fresh ones to compare.
        let mut fingerprints: Vec<(i64, Vec<u32>)> = Vec::new();
        for (item_id, part) in &episodes {
            if intro_unchanged(*item_id, part) {
                println!("  item {item_id}: unchanged, skipping");
                continue;
            }
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
            return Ok(0);
        }

        // compare all pairs; each episode collects its own match times
        let mut matches: HashMap<i64, Vec<(f64, f64)>> = HashMap::new();
        for i in 0..fingerprints.len() {
            for j in (i + 1)..fingerprints.len() {
                let (id_a, fp_a) = &fingerprints[i];
                let (id_b, fp_b) = &fingerprints[j];
                if let Some(m) = find_common_intro(fp_a, fp_b) {
                    matches.entry(*id_a).or_default().push((m.start_a, m.end_a));
                    matches.entry(*id_b).or_default().push((m.start_b, m.end_b));
                }
            }
        }

        // write each episode's own consensus times
        for (item_id, ms) in &matches {
            if ms.is_empty() {
                continue;
            }
            let mut starts: Vec<f64> = ms.iter().map(|m| m.0).collect();
            let mut ends: Vec<f64> = ms.iter().map(|m| m.1).collect();
            starts.sort_by(|a, b| a.partial_cmp(b).unwrap());
            ends.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let start = starts[starts.len() / 2].max(0.0);
            let end = ends[ends.len() / 2];

            let start_ms = (start * 1000.0) as i64;
            let end_ms = (end * 1000.0) as i64;
            db.write_marker(*item_id, MARKER_INTRO, start_ms, end_ms)?;
            if let Some(part) = episodes.iter().find(|(id, _)| id == item_id)
                && let Some(cache) = intro_cache()
            {
                let key = media_key(&part.1);
                cache.record_marker(
                    *item_id,
                    MARKER_INTRO,
                    start_ms,
                    end_ms,
                    &crate::runtime::marker_digest(&key),
                    std::time::SystemTime::now(),
                )?;
            }
            written += 1;
            println!(
                "  item {item_id}: intro {start:.1}-{end:.1}s ({} pairs)",
                ms.len()
            );
        }
    }
    Ok(written)
}

/// Whether this episode's intro marker still stands.
///
/// True when the ledger has a row and the strm target is unchanged, or
/// when the marker was set by hand or stopped. A ledger that cannot be
/// opened reports false, so detection runs as it did before the ledger.
fn intro_unchanged(item_id: i64, part: &crate::plexdb::Part) -> bool {
    let Some(cache) = intro_cache() else {
        return false;
    };
    let digest = crate::runtime::marker_digest(&media_key(part));
    cache.marker_decision(item_id, MARKER_INTRO, &digest).ok()
        == Some(crate::runtime::MarkerDecision::Skip)
}

/// Duration and size, the pair the content ledger matches on. A part
/// that has not been analysed yet has nothing stable, so its key is
/// empty and detection runs.
fn media_key(part: &crate::plexdb::Part) -> String {
    if part.duration <= 0 || part.size <= 0 {
        return String::new();
    }
    format!("d={}\ns={}\n", part.duration, part.size)
}

/// The ledger. The path is the one the runtime service uses, overridable
/// so a scan outside the container can point at its own cache.
fn intro_cache() -> Option<crate::runtime::Cache> {
    crate::runtime::Cache::open(crate::runtime::cache_path()).ok()
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
    match linkres::resolve(&target.value, ua).await {
        Ok(link) => Some(link.url),
        Err(err) => {
            // the daemon's ledger is the only place a later scan looks,
            // so a failure here has to be booked or it is retried forever
            if let Some(sock) = sock
                && let Ok(client) = Client::new(sock)
            {
                let _ = client.dead(&target.value, err.is_permanent()).await;
            }
            None
        }
    }
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
    fn finds_identical_intro_at_offset_zero() {
        let common: Vec<u32> = (0..150).map(|i| 0xDEAD_BEEF ^ i).collect();
        let tail_a: Vec<u32> = (0..50).map(|i| 0x1234_5678u32.wrapping_add(i)).collect();
        let tail_b: Vec<u32> = (0..50).map(|i| 0x9ABC_DEF0u32.wrapping_sub(i)).collect();

        let a: Vec<u32> = common.iter().chain(tail_a.iter()).cloned().collect();
        let b: Vec<u32> = common.iter().chain(tail_b.iter()).cloned().collect();

        let m = find_common_intro(&a, &b).expect("identical head must match");
        assert_eq!(m.offset, 0);
        assert!(m.start_a < 1.0, "A starts near zero: {}", m.start_a);
        assert!(m.end_a > 15.0, "at least 15 seconds in A: {}", m.end_a);
        assert!(
            (m.start_a - m.start_b).abs() < 0.1,
            "same position in both at offset 0"
        );
        assert!(m.score > 0.95, "near-perfect similarity: {}", m.score);
    }

    #[test]
    fn finds_shifted_intro() {
        // A: 50 junk + 150 common + 50 junk
        // B: 100 junk + 150 common + 50 junk (common starts 50 entries later)
        let common: Vec<u32> = (0..150).map(|i| 0xFEED_FACE ^ i).collect();
        let junk: Vec<u32> = (0..50)
            .map(|i| 0x1111_2222u32.wrapping_mul(i + 1))
            .collect();
        let junk2: Vec<u32> = (0..100)
            .map(|i| 0x3333_4444u32.wrapping_mul(i + 1))
            .collect();

        let a: Vec<u32> = junk
            .iter()
            .chain(common.iter())
            .chain(junk.iter())
            .cloned()
            .collect();
        let b: Vec<u32> = junk2
            .iter()
            .chain(common.iter())
            .chain(junk.iter())
            .cloned()
            .collect();

        let m = find_common_intro(&a, &b).expect("shifted common block must be found");
        assert!(m.offset > 0, "B's intro starts later: {}", m.offset);
        assert!(m.duration() > 15.0, "long enough: {}", m.duration());
        // A's intro starts around 50 entries ≈ 6.2 seconds
        assert!(
            (6.0..7.0).contains(&m.start_a),
            "A's intro at ~6.2s: {}",
            m.start_a
        );
        // B's intro starts around 100 entries ≈ 12.4 seconds
        assert!(
            (11.5..13.0).contains(&m.start_b),
            "B's intro at ~12.4s: {}",
            m.start_b
        );
    }

    #[test]
    fn no_match_when_different() {
        // Knuth multiplicative hash: well-distributed, no linear structure
        let a: Vec<u32> = (0..200)
            .map(|i| (i as u32).wrapping_mul(2654435761))
            .collect();
        let b: Vec<u32> = (0..200).map(|i| (i as u32).wrapping_mul(40503)).collect();
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

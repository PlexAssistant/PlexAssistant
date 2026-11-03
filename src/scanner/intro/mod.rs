//! Intro detection: fingerprint the head of each strm episode in a
//! season, find the audio the episodes share, and write an intro marker
//! for each episode's own position of it.

mod fingerprint;
mod matching;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

use super::{Options, resolve_link};
use crate::linkres;
use crate::plexdb::{Db, MARKER_INTRO, Part};
use crate::runtime::{Cache, MarkerDecision, marker_digest};
use fingerprint::extract_fingerprint;
use matching::find_common_intro;

/// Detects intros for every season in a library section: fingerprint
/// each strm episode's head, cross-compare within the season, and write
/// intro markers for the consensus segment. Returns the number of
/// markers written.
pub async fn detect_section_intros(
    db: &mut Db,
    section_id: i64,
    ffmpeg: &str,
    fpcalc: &str,
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
    let episodes = db.strm_episodes_by_season(season_id)?;
    if episodes.len() < 2 {
        return Ok(0); // need at least two episodes to compare
    }
    println!("season {season_id}: {} strm episode(s)", episodes.len());

    // The ledger. A ledger that cannot be opened skips nothing, so
    // detection runs as it did before the ledger.
    let cache = Cache::open(crate::runtime::cache_path()).ok();
    let opts = Options {
        user_agent: ua.to_owned(),
        runtime_sock: sock.cloned(),
        ..Default::default()
    };
    let tmpdir = tempfile::tempdir().map_err(|e| format!("tmpdir: {e}"))?;

    // fingerprint each episode's head. An episode whose content has
    // not changed, or whose marker was set by hand, is not fingerprinted
    // again; the season still needs two fresh ones to compare.
    let mut fingerprints: Vec<(i64, Vec<u32>)> = Vec::new();
    for (item_id, part) in &episodes {
        if cache
            .as_ref()
            .is_some_and(|c| intro_unchanged(c, *item_id, part))
        {
            println!("  item {item_id}: unchanged, skipping");
            continue;
        }
        let Some(link) = resolve(part, &opts).await else {
            println!("  item {item_id}: resolve failed, skipping");
            continue;
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
    for (i, (id_a, fp_a)) in fingerprints.iter().enumerate() {
        for (id_b, fp_b) in &fingerprints[i + 1..] {
            if let Some(m) = find_common_intro(fp_a, fp_b) {
                matches.entry(*id_a).or_default().push((m.start_a, m.end_a));
                matches.entry(*id_b).or_default().push((m.start_b, m.end_b));
            }
        }
    }

    // write each episode's own consensus times
    let mut written = 0;
    for (item_id, ms) in &matches {
        let mut starts: Vec<f64> = ms.iter().map(|m| m.0).collect();
        let mut ends: Vec<f64> = ms.iter().map(|m| m.1).collect();
        starts.sort_by(f64::total_cmp);
        ends.sort_by(f64::total_cmp);
        let start = starts[starts.len() / 2].max(0.0);
        let end = ends[ends.len() / 2];

        let start_ms = (start * 1000.0) as i64;
        let end_ms = (end * 1000.0) as i64;
        db.write_marker(*item_id, MARKER_INTRO, start_ms, end_ms)?;
        if let Some((_, part)) = episodes.iter().find(|(id, _)| id == item_id)
            && let Some(cache) = &cache
        {
            cache.record_marker(
                *item_id,
                MARKER_INTRO,
                start_ms,
                end_ms,
                &marker_digest(&part.media_key()),
                SystemTime::now(),
            )?;
        }
        written += 1;
        println!(
            "  item {item_id}: intro {start:.1}-{end:.1}s ({} pairs)",
            ms.len()
        );
    }
    Ok(written)
}

/// Whether this episode's intro marker still stands.
///
/// True when the ledger has a row and the part's duration and size are
/// unchanged, or when the marker was set by hand or stopped.
fn intro_unchanged(cache: &Cache, item_id: i64, part: &Part) -> bool {
    let digest = marker_digest(&part.media_key());
    cache.marker_decision(item_id, MARKER_INTRO, &digest).ok() == Some(MarkerDecision::Skip)
}

/// Resolves a strm part to a direct link, through the daemon when one is
/// reachable. A failure is booked there, so a later scan backs off.
async fn resolve(part: &Part, opts: &Options) -> Option<String> {
    let target = linkres::parse_strm(&part.file).ok()?;
    if !target.is_remote {
        return None;
    }
    resolve_link(&target.value, opts).await.map(|link| link.url)
}

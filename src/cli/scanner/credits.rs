//! Plex's credits-detection invocation (`-C -f <file>`), answered with
//! the assistant's detectors for strm files.

use std::ffi::{OsStr, OsString};

use super::exec_original;
use crate::cli::{CliResult, env_or};
use crate::linkres::{self, DEFAULT_UA};
use crate::plexdb::{self, Db, Part};
use crate::runtime::{self, DEFAULT_SOCK};
use crate::scanner::{self, Options, credits};

/// The -f target of a credits-detection invocation, when -C is present.
pub(super) fn credits_target(args: &[OsString]) -> Option<OsString> {
    if !args.iter().any(|a| a == "-C") {
        return None;
    }
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == "-f" {
            return it.next().cloned();
        }
    }
    None
}

/// Duration and size of the item's first analysed part. Empty when
/// nothing has been probed yet, so the first credits run always detects.
fn credits_key(db: &Db, item: i64) -> Result<String, rusqlite::Error> {
    let parts = db.parts_by_item(item)?;
    Ok(parts
        .iter()
        .map(Part::media_key)
        .find(|key| !key.is_empty())
        .unwrap_or_default())
}

/// Credits detection for one file: find the item, resolve a strm target
/// through the daemon, run the detectors `ASSISTANT_CREDITS` picks and
/// write the marker. Local files and unknown paths keep the official path.
pub(super) async fn run_credits(file: &OsStr) -> CliResult {
    let file_str = file.to_string_lossy().into_owned();
    let opts = Options {
        ffprobe: env_or("ASSISTANT_FFPROBE", super::super::DEFAULT_FFPROBE),
        user_agent: DEFAULT_UA.into(),
        runtime_sock: Some(env_or("ASSISTANT_SOCK", DEFAULT_SOCK).into()),
        cache: Some(runtime::cache_path()),
    };

    let mut db = Db::open(plexdb::DEFAULT_PATH)?;
    let Some(item) = db.item_id_by_part_file(&file_str)? else {
        return Err(exec_original().into());
    };
    let target = match linkres::parse_strm(file) {
        Ok(target) if target.is_remote => target,
        _ => return Err(exec_original().into()), // local or unreadable: official path
    };

    // The ledger is consulted before resolving: an unchanged file, a
    // hand-set marker, or a stopped one must not spend a detection run.
    // The key is the part's duration and size, the same pair the content
    // ledger matches on, so a re-added item of the same file still skips.
    // A missing cache must not stop detection: the marker is still
    // written to Plex.
    let digest = runtime::marker_digest(&credits_key(&db, item)?);
    let cache = runtime::Cache::open(runtime::cache_path()).ok();
    if let Some(cache) = &cache
        && cache.marker_decision(item, plexdb::MARKER_CREDITS, &digest)?
            == runtime::MarkerDecision::Skip
    {
        println!("credits: item {item} unchanged, skipping");
        return Ok(0);
    }

    let Some(link) = scanner::resolve_link(&target.value, &opts).await else {
        println!("credits: resolve failed for {file_str}");
        return Ok(0); // nothing to detect; PMS counts this a success
    };
    let ffmpeg = env_or("ASSISTANT_FFMPEG", super::super::DEFAULT_FFMPEG);
    // one probe for every detector: each seeks into the tail from it
    let duration = scanner::probe(&opts.ffprobe, &link.url, &opts.user_agent)
        .await
        .map(|p| scanner::duration_ms(&p.format.duration) as f64 / 1000.0)
        .unwrap_or(0.0);
    let detector = credits::Detector::from_env();
    match credits::detect(detector, &ffmpeg, &link.url, &opts.user_agent, duration).await {
        Some((start, end)) => {
            db.write_marker(item, plexdb::MARKER_CREDITS, start, end)?;
            if let Some(cache) = &cache {
                cache.record_marker(
                    item,
                    plexdb::MARKER_CREDITS,
                    start,
                    end,
                    &digest,
                    std::time::SystemTime::now(),
                )?;
            }
            println!("credits: marker {start}-{end} ms on item {item}");
        }
        None => println!("credits: no window found for {file_str}"),
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(args: &[&str]) -> Option<OsString> {
        credits_target(&args.iter().map(OsString::from).collect::<Vec<_>>())
    }

    #[test]
    fn needs_dash_c_and_a_file() {
        assert_eq!(
            target(&["-C", "-f", "/x/a.strm", "--creditsTempDataPath", "/t"]),
            Some("/x/a.strm".into())
        );
        assert_eq!(target(&["-f", "/x/a.strm"]), None, "no -C");
        assert_eq!(target(&["-C", "-f"]), None, "no file after -f");
    }
}

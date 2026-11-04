//! The scan pipeline for one item: resolve each strm part's direct link,
//! probe it, and write the media rows Plex would have written.

mod link;
mod selections;
#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::linkres::{self, Link};
use crate::localize::Localize;
use crate::plexdb::{Db, Part, WriteError, WriteSpec};
use crate::runtime::{Cache, Identity, PartFailure};

use super::probe::ProbeResult;
use super::spec::build_spec;
use link::probe_and_heal;
use selections::{apply_show_choice, keep_selections, retarget_kept};

pub use link::resolve_link;

/// Configures one analysis run.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub ffprobe: String,
    pub user_agent: String,
    /// daemon socket; None or unreachable = resolve directly
    pub runtime_sock: Option<PathBuf>,
    /// assistant cache; None falls back to `ASSISTANT_CACHE`, then the
    /// default path inside the container
    pub cache: Option<PathBuf>,
    /// Chinese sort titles and tag labels. Off rewrites nothing.
    pub localize: Localize,
    /// Drop the locks this pass wrote instead of writing.
    pub localize_unlock: bool,
}

impl Options {
    /// The assistant cache, when it can be opened. Callers open it once
    /// per item: every open re-runs the schema statements.
    fn open_cache(&self) -> Option<Cache> {
        let path = self
            .cache
            .clone()
            .unwrap_or_else(crate::runtime::cache_path);
        Cache::open(path).ok()
    }
}

/// The targets that got a fresh link this run. Every target gets at most
/// one refresh per round, however many parts point at it and however many
/// items are probed at once.
pub type Refreshed = Arc<Mutex<HashSet<String>>>;

#[derive(Debug, thiserror::Error)]
pub enum AnalyzeError {
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error("item {item} part {part}: write: {source}")]
    Write {
        item: i64,
        part: i64,
        source: WriteError,
    },
}

/// Reports whether a part file is a .strm (case-insensitive).
pub fn is_strm(file: &str) -> bool {
    file.to_lowercase().ends_with(".strm")
}

/// Runs the pipeline for every strm part of one metadata item: resolve the
/// direct link, probe it, write the results. Non-strm parts and failures
/// are skipped without aborting the run; only a failed write stops it.
pub async fn analyze_item(
    db: &mut Db,
    item_id: i64,
    opts: &Options,
    refreshed: &Refreshed,
) -> Result<usize, AnalyzeError> {
    let parts = db.parts_by_item(item_id)?;
    let probed = probe_item(parts, item_id, opts, refreshed).await;
    let written = write_item(db, item_id, probed, opts)?;
    localize_item(db, item_id, opts);
    Ok(written)
}

/// One part whose probe succeeded and is ready to be written.
pub struct Probed {
    part: Part,
    spec: WriteSpec,
}

/// Resolves and probes every strm part of one item, without touching the
/// library. The library read happens first, so the rest can run alongside
/// other items; the caller writes the results back on its own connection.
/// Failures are logged and skipped.
pub async fn probe_item(
    parts: Vec<Part>,
    item_id: i64,
    opts: &Options,
    refreshed: &Refreshed,
) -> Vec<Probed> {
    let cache = opts.open_cache();
    let cache = cache.as_ref();
    let mut out = Vec::new();
    for part in parts.into_iter().filter(|p| is_strm(&p.file)) {
        let target = match linkres::parse_strm(&part.file) {
            Ok(target) => target,
            Err(e) => {
                println!("item {item_id} part {}: {e}", part.id);
                note_part(cache, part.id, PartFailure::Permanent);
                continue;
            }
        };
        if !target.is_remote {
            continue; // local path inside a strm: not ours
        }
        if cache.is_some_and(|c| c.part_skipped(part.id).unwrap_or(false)) {
            println!("item {item_id} part {}: skipped", part.id);
            continue;
        }
        // A part that already carries media info can be recognised before
        // any probe: duration and size are what the content ledger stores.
        // A hit means this file was probed before, under some other path or
        // item, and the rows can be copied instead of probed again.
        if let Some(spec) = cache.and_then(|c| reused_spec(c, &part)) {
            println!("item {item_id} part {}: same content, reusing", part.id);
            out.push(Probed { part, spec });
            continue;
        }
        let Some(link) = resolve_link(&target.value, opts).await else {
            println!("item {item_id} part {}: resolve failed", part.id);
            note_part(cache, part.id, PartFailure::Temporary);
            continue;
        };
        let Some((probed, link)) =
            probe_and_heal(&link, &target.value, opts, refreshed, item_id, part.id).await
        else {
            note_part(cache, part.id, PartFailure::Temporary);
            continue;
        };
        if let Some(cache) = cache {
            let _ = cache.clear_part_failure(part.id);
        }
        let spec = remember_probe(cache, &part, &probed, &link);
        out.push(Probed { part, spec });
    }
    out
}

/// Turns a fresh probe into the row to write, and remembers it when the
/// probe has enough stable facts to recognise the file later.
///
/// The key is the probe, not the strm path or the signed link. A later
/// part of the same file — another path, a re-added item, a resigned URL —
/// finds the row and is written without a second probe only once that
/// lookup exists. The first sight of a file always probes: nothing stable
/// is known before ffprobe runs.
fn remember_probe(
    cache: Option<&Cache>,
    part: &Part,
    probed: &ProbeResult,
    link: &Link,
) -> WriteSpec {
    let spec = build_spec(part, probed, link);
    if let Some(cache) = cache
        && let Some(identity) = Identity::from_probe(probed)
    {
        let _ = cache.remember_content(&identity.key(), &spec, SystemTime::now());
    }
    spec
}

/// The analysis already stored for a part whose library row already has
/// a duration and a size, copied onto this part. Parts that have never
/// been analysed return nothing.
fn reused_spec(cache: &Cache, part: &Part) -> Option<WriteSpec> {
    if part.duration <= 0 || part.size <= 0 {
        return None;
    }
    let mut spec = cache
        .content_for_media(part.duration, part.size)
        .ok()??
        .spec;
    spec.media_item_id = part.media_item_id;
    spec.part_id = part.id;
    Some(spec)
}

fn note_part(cache: Option<&Cache>, part_id: i64, kind: PartFailure) {
    if let Some(cache) = cache {
        let _ = cache.record_part_failure(part_id, kind, SystemTime::now());
    }
}

/// Writes the probed parts of one item. A failed write stops the run.
///
/// A rewrite replaces the stream rows, which drops the ids a saved audio or
/// subtitle choice pointed at. Those ids are read first and pointed at the
/// new rows afterwards. A choice already remembered for this account and
/// show is applied to a part that has none.
pub fn write_item(
    db: &mut Db,
    item_id: i64,
    probed: Vec<Probed>,
    opts: &Options,
) -> Result<usize, AnalyzeError> {
    let show = db.show_of_item(item_id).ok().flatten();
    let cache = opts.open_cache();
    for row in &probed {
        let kept = keep_selections(db, row.part.id);
        db.update_media(&row.spec)
            .map_err(|source| AnalyzeError::Write {
                item: item_id,
                part: row.part.id,
                source,
            })?;
        retarget_kept(db, row.part.id, show, &kept, cache.as_ref());
        if let (Some(show), Some(cache)) = (show, cache.as_ref()) {
            apply_show_choice(db, cache, show, row.part.id, &kept);
        }
    }
    Ok(probed.len())
}

/// Rewrites the sort title and tag labels `opts.localize` names, or drops
/// those locks when `opts.localize_unlock` is set. A failure is logged and
/// does not fail the scan: the media rows are already written.
pub fn localize_item(db: &mut Db, item_id: i64, opts: &Options) {
    if opts.localize_unlock {
        match (db.unlock_sort(item_id), db.unlock_tags(item_id)) {
            (Ok(sort), Ok(tags)) if sort || tags => println!("item {item_id}: unlocked"),
            (Err(e), _) | (_, Err(e)) => println!("item {item_id}: localize: {e}"),
            _ => {}
        }
        return;
    }
    if opts.localize.sort {
        match db.localize_sort(item_id) {
            Ok(true) => println!("item {item_id}: sort"),
            Ok(false) => {}
            Err(e) => println!("item {item_id}: localize: {e}"),
        }
    }
    if opts.localize.tags {
        match db.localize_tags(item_id) {
            Ok(n) if n > 0 => println!("item {item_id}: tags"),
            Ok(_) => {}
            Err(e) => println!("item {item_id}: localize: {e}"),
        }
    }
}

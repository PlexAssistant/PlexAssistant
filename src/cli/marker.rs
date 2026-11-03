//! Inspects and edits the marker ledger.
//!
//! Detection reads the same rows. A manual row is never replaced by a
//! later scan, and a stopped one is not detected again until retried.

use std::path::PathBuf;
use std::time::SystemTime;

use clap::Subcommand;

use super::CliResult;
use crate::plexdb::{self, Db};
use crate::runtime::{Cache, MarkerOrigin, marker_digest};

/// Shows and edits the markers detection booked.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// ledger database; the assistant's cache
    #[arg(long, default_value = "/config/assistant/cache.db")]
    cache: PathBuf,
    /// library database, used to resolve an item when setting a marker
    #[arg(long, default_value = crate::plexdb::DEFAULT_PATH)]
    library_db: PathBuf,
    #[command(subcommand)]
    action: Action,
}

#[derive(Debug, Subcommand)]
enum Action {
    /// Lists every booked marker.
    List,
    /// Sets a marker by hand. Later scans will not replace it.
    Set {
        #[arg(long)]
        item: i64,
        /// `intro` or `credits`
        #[arg(long)]
        kind: String,
        /// start, in milliseconds
        #[arg(long)]
        start: i64,
        /// end, in milliseconds
        #[arg(long)]
        end: i64,
    },
    /// Forgets a row so the next scan detects it again.
    Retry {
        #[arg(long)]
        item: i64,
        #[arg(long)]
        kind: String,
    },
    /// Stops detection for one item and marker type.
    Stop {
        #[arg(long)]
        item: i64,
        #[arg(long)]
        kind: String,
    },
}

pub fn run(args: Args) -> CliResult {
    let cache = Cache::open(&args.cache)?;
    match args.action {
        Action::List => list(&cache)?,
        Action::Set {
            item,
            kind,
            start,
            end,
        } => {
            let kind_id = kind_id(&kind)?;
            let mut db = Db::open(&args.library_db)?;
            db.write_marker(item, kind_id, start, end)?;
            cache.override_marker(
                item,
                kind_id,
                start,
                end,
                &digest_of(&db, item)?,
                SystemTime::now(),
            )?;
            println!("marker: item {item} {kind} set to {start}-{end} ms");
        }
        Action::Retry { item, kind } => {
            let kind_id = kind_id(&kind)?;
            if cache.retry_marker(item, kind_id)? {
                println!("marker: item {item} {kind} will be detected again");
            } else {
                println!("marker: item {item} {kind} has no row");
            }
        }
        Action::Stop { item, kind } => {
            let kind_id = kind_id(&kind)?;
            if cache.stop_marker(item, kind_id, SystemTime::now())? {
                println!("marker: item {item} {kind} stopped");
            } else {
                println!("marker: item {item} {kind} has no row");
            }
        }
    }
    Ok(0)
}

fn list(cache: &Cache) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let rows = cache.markers()?;
    if rows.is_empty() {
        println!("marker: none booked");
        return Ok(());
    }
    for row in rows {
        println!(
            "item {} {} {}-{} ms {} {}",
            row.item_id,
            kind_name(row.marker_type),
            row.start_ms,
            row.end_ms,
            origin_name(row.origin),
            row.digest
        );
    }
    Ok(())
}

/// The digest of the item's strm target, so a hand-set marker compares
/// against the same content a scan would.
fn digest_of(db: &Db, item: i64) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let parts = db.parts_by_item(item)?;
    let target = parts
        .iter()
        .find_map(|part| crate::linkres::parse_strm(&part.file).ok())
        .filter(|target| target.is_remote)
        .map(|target| target.value)
        .unwrap_or_default();
    Ok(marker_digest(&target))
}

fn kind_id(kind: &str) -> Result<i64, String> {
    match kind {
        "intro" => Ok(plexdb::MARKER_INTRO),
        "credits" => Ok(plexdb::MARKER_CREDITS),
        other => Err(format!("marker kind must be intro or credits, not {other}")),
    }
}

fn kind_name(kind: i64) -> &'static str {
    match kind {
        plexdb::MARKER_INTRO => "intro",
        plexdb::MARKER_CREDITS => "credits",
        _ => "other",
    }
}

fn origin_name(origin: MarkerOrigin) -> &'static str {
    match origin {
        MarkerOrigin::Auto => "auto",
        MarkerOrigin::Manual => "manual",
        MarkerOrigin::Stopped => "stopped",
    }
}

//! The Plex Scanner replacement: strm analysis and credits detection,
//! with every other mode handed to the preserved original.

mod credits;
mod flags;

use std::ffi::{OsStr, OsString};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{CliResult, env_or};
use crate::plexdb::Db;
use crate::runtime;
use crate::scanner::{self, Options, Refreshed, is_strm};
use credits::{credits_target, run_credits};
use flags::parse_flags;

/// Run as the Plex Scanner replacement. Item and section analyze
/// invocations run our strm pipeline; every other mode (matcher, credits,
/// chapter thumbs) is handed to the preserved original.
#[derive(Debug, clap::Args)]
#[command(disable_help_flag = true)]
pub struct Args {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
    args: Vec<OsString>,
}

pub async fn run(args: Args) -> CliResult {
    // credits detection: Plex invokes "-C -f <file> --creditsTempDataPath …"
    if let Some(file) = credits_target(&args.args) {
        return run_credits(&file).await;
    }

    let flags = parse_flags(&args.args)?;
    if !flags.analyze || (flags.item == 0 && flags.section == 0) {
        return Err(exec_original().into());
    }

    let opts = Options {
        ffprobe: flags.ffprobe,
        user_agent: flags.ua,
        runtime_sock: Some(flags.runtime_sock).filter(|s| !s.as_os_str().is_empty()),
        cache: Some(runtime::cache_path()),
        localize: plex_localize::Localize::from_env(),
        localize_unlock: plex_localize::unlock_from_env(),
    };
    let mut db = Db::open(&flags.library_db)?;
    if flags.item > 0 {
        analyze_one(&mut db, flags.item, &opts).await
    } else {
        analyze_section(&mut db, flags.section, &opts).await
    }
}

async fn analyze_one(db: &mut Db, item: i64, opts: &Options) -> CliResult {
    let parts = db.parts_by_item(item)?;
    if !parts.iter().any(|p| is_strm(&p.file)) {
        return Err(exec_original().into()); // local media: keep the official path
    }
    warn_missing_ffprobe(&opts.ffprobe);
    let n = scanner::analyze_item(db, item, opts, &Refreshed::default()).await?;
    println!("item {item}: analyzed {n} strm part(s)");
    // A single episode is enough to know its season. Intros need two
    // episodes to compare, so the season is only visited when it has them;
    // episodes already booked are not fingerprinted again.
    if let Some(season) = db.season_of_item(item)? {
        detect_intros(db, &[season], opts).await;
    }
    Ok(0)
}

/// Intro detection for the seasons a scan just touched.
///
/// Failures are logged. A season with fewer than two strm episodes, or
/// one whose episodes are already booked, writes nothing.
async fn detect_intros(db: &mut Db, seasons: &[i64], opts: &Options) {
    let ffmpeg = env_or("ASSISTANT_FFMPEG", super::DEFAULT_FFMPEG);
    let fpcalc = env_or("ASSISTANT_FPCALC", "fpcalc");
    for &season in seasons {
        match scanner::intro::detect_season_intros(
            db,
            season,
            &ffmpeg,
            &fpcalc,
            &opts.user_agent,
            opts.runtime_sock.as_ref(),
        )
        .await
        {
            Ok(0) => {}
            Ok(n) => println!("season {season}: {n} intro marker(s)"),
            Err(e) => println!("season {season}: intro: {e}"),
        }
    }
}

/// How many items a section scan probes at once. Probing waits on the
/// network, so a few in flight beats one at a time; more than a handful
/// just crowds the one drive most of a library lives on. Writes stay on
/// the single library connection and run one item at a time.
const PROBE_CONCURRENCY: usize = 3;

async fn analyze_section(db: &mut Db, section: i64, opts: &Options) -> CliResult {
    let ids = db.items_by_section(section)?;
    let mut pending = Vec::new();
    for &id in &ids {
        let parts = db.parts_by_item(id)?;
        if parts.iter().any(|p| is_strm(&p.file)) {
            pending.push((id, parts));
        }
    }

    // one refresh budget per target for the whole run
    if !pending.is_empty() {
        warn_missing_ffprobe(&opts.ffprobe);
    }
    let refreshed = Refreshed::default();
    let mut in_flight = tokio::task::JoinSet::new();
    let mut rest = pending.into_iter();
    loop {
        while in_flight.len() < PROBE_CONCURRENCY
            && let Some((id, parts)) = rest.next()
        {
            let opts = opts.clone();
            let refreshed = refreshed.clone();
            in_flight.spawn(async move {
                let probed = scanner::probe_item(parts, id, &opts, &refreshed).await;
                (id, probed)
            });
        }
        let Some(joined) = in_flight.join_next().await else {
            break;
        };
        let (id, probed) = match joined {
            Ok(row) => row,
            Err(e) => {
                println!("probe task failed: {e}");
                continue;
            }
        };
        if let Err(e) = scanner::write_item(db, id, probed, opts) {
            println!("item {id}: {e}");
            continue;
        }
        scanner::localize_item(db, id, opts).await;
    }
    if let Err(e) = db.checkpoint() {
        println!("checkpoint: {e}");
    }
    match db.seasons_by_section(section) {
        Ok(seasons) => detect_intros(db, &seasons, opts).await,
        Err(e) => println!("section {section}: seasons: {e}"),
    }
    println!("section {section}: {} item(s) processed", ids.len());
    Ok(0)
}

/// Says once when the configured ffprobe is not there. The default is the
/// path inside the image; outside the image nothing sits there until
/// `ASSISTANT_FFPROBE` names the host's own.
fn warn_missing_ffprobe(ffprobe: &str) {
    if !Path::new(ffprobe).is_file() {
        println!("ffprobe not found at {ffprobe}; set ASSISTANT_FFPROBE to name the one to use");
    }
}

/// Replaces this process with the preserved Plex binary so unhandled
/// modes keep their official behavior. Only returns on failure.
pub(super) fn exec_original() -> String {
    let argv: Vec<OsString> = std::env::args_os().collect();
    let mut original = argv.first().cloned().unwrap_or_default();
    original.push(crate::install::ORIGINAL_SUFFIX);
    let original = PathBuf::from(original);
    if !original.exists() {
        return format!("original scanner not found at {}", original.display());
    }
    exec(&original, &argv[1..]).to_string()
}

fn exec(path: &Path, args: &[OsString]) -> std::io::Error {
    Command::new(path).arg0(OsStr::new(path)).args(args).exec()
}

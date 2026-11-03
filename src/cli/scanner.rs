use std::ffi::{OsStr, OsString};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{CliResult, env_or};
use crate::linkres::{self, DEFAULT_UA};
use crate::plexdb::{self, Db};
use crate::runtime::{self, DEFAULT_SOCK};
use crate::scanner::{self, Options, Refreshed, credits, is_strm};

/// Run as the Plex Scanner replacement. Item and section analyze
/// invocations run our strm pipeline; every other mode (matcher, credits,
/// chapter thumbs) is handed to the preserved original.
#[derive(Debug, clap::Args)]
#[command(disable_help_flag = true)]
pub struct Args {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
    args: Vec<OsString>,
}

/// The flags we act on; Plex passes many more, which are ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ScanFlags {
    analyze: bool,
    section: i64,
    item: i64,
    library_db: PathBuf,
    ffprobe: String,
    ua: String,
    runtime_sock: PathBuf,
}

impl Default for ScanFlags {
    fn default() -> Self {
        Self {
            analyze: false,
            section: 0,
            item: 0,
            library_db: plexdb::DEFAULT_PATH.into(),
            ffprobe: env_or("ASSISTANT_FFPROBE", "ffprobe"),
            ua: DEFAULT_UA.into(),
            runtime_sock: DEFAULT_SOCK.into(),
        }
    }
}

/// Parses Plex's scanner argv with pflag's rules for whitelisted unknown
/// flags: `--name=value` or `--name value` (a known flag takes the next
/// argument whatever it is), and an unknown flag without `=` swallows the
/// next argument unless that looks like a flag, so a later `--item N` is
/// never mistaken for its value.
fn parse_flags(args: &[OsString]) -> Result<ScanFlags, String> {
    let mut flags = ScanFlags::default();
    let mut rest = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .peekable();
    while let Some(arg) = rest.next() {
        if arg == "--" {
            break; // positional arguments only from here on
        }
        // a lone "-" is positional, like everything not starting with '-'
        let Some(flag) = arg
            .strip_prefix("--")
            .or_else(|| arg.strip_prefix('-'))
            .filter(|f| !f.is_empty())
        else {
            continue; // positional: ignored
        };
        let (name, inline) = match flag.split_once('=') {
            Some((name, value)) => (name, Some(value.to_owned())),
            None => (flag, None),
        };
        let looks_like_value = |next: &Option<&String>| next.is_some_and(|n| !n.starts_with('-'));
        let mut value = |name: &str| {
            inline
                .clone()
                .or_else(|| rest.next()) // a known flag takes the next arg as-is
                .ok_or_else(|| format!("flag needs an argument: --{name}"))
        };
        let int = |name: &str, v: String| {
            v.parse::<i64>()
                .map_err(|_| format!("invalid argument {v:?} for --{name}"))
        };
        match name {
            "analyze" => {
                flags.analyze = match inline.as_deref() {
                    None | Some("true" | "1" | "t" | "T" | "TRUE" | "True") => true,
                    Some("false" | "0" | "f" | "F" | "FALSE" | "False") => false,
                    Some(v) => return Err(format!("invalid argument {v:?} for --analyze")),
                }
            }
            "section" => flags.section = int(name, value(name)?)?,
            "item" => flags.item = int(name, value(name)?)?,
            "library-db" => flags.library_db = value(name)?.into(),
            "ffprobe" => flags.ffprobe = value(name)?,
            "ua" => flags.ua = value(name)?,
            "runtime-sock" => flags.runtime_sock = value(name)?.into(),
            _ => {
                if inline.is_none() && looks_like_value(&rest.peek()) {
                    rest.next();
                }
            }
        }
    }
    Ok(flags)
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
    };
    let mut db = Db::open(&flags.library_db)?;
    if flags.item > 0 {
        analyze_one(&mut db, flags.item, &opts).await
    } else {
        analyze_section(&mut db, flags.section, &opts).await
    }
}

/// The -f target of a credits-detection invocation, when -C is present.
fn credits_target(args: &[OsString]) -> Option<OsString> {
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
        .find(|part| part.duration > 0 && part.size > 0)
        .map(|part| format!("d={}\ns={}\n", part.duration, part.size))
        .unwrap_or_default())
}

/// The assistant's own ledger, when its cache can be opened. A missing
/// cache must not stop detection: the marker is still written to Plex.
fn marker_cache() -> Option<runtime::Cache> {
    runtime::Cache::open(runtime::cache_path()).ok()
}

/// Credits detection for one file: find the item, resolve a strm target
/// through the daemon, run the detectors `ASSISTANT_CREDITS` picks and
/// write the marker. Local files and unknown paths keep the official path.
async fn run_credits(file: &OsStr) -> CliResult {
    let file_str = file.to_string_lossy().into_owned();
    let opts = Options {
        ffprobe: env_or("ASSISTANT_FFPROBE", "ffprobe"),
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
    let digest = runtime::marker_digest(&credits_key(&db, item)?);
    if let Some(cache) = marker_cache()
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
    let ffmpeg = env_or("ASSISTANT_FFMPEG", "ffmpeg");
    let duration = input_duration(&opts.ffprobe, &link.url, &opts.user_agent).await;
    let marker = credits::detect(
        credits::Detector::from_env(),
        &ffmpeg,
        &opts.ffprobe,
        &link.url,
        &opts.user_agent,
        duration,
    )
    .await;
    match marker {
        Some((start, end)) => {
            db.write_marker(item, plexdb::MARKER_CREDITS, start, end)?;
            if let Some(cache) = marker_cache() {
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

/// Duration in seconds via ffprobe.
async fn input_duration(ffprobe: &str, input: &str, ua: &str) -> f64 {
    use scanner::duration_ms;
    scanner::probe(ffprobe, input, ua)
        .await
        .map(|p| duration_ms(&p.format.duration) as f64 / 1000.0)
        .unwrap_or(0.0)
}

async fn analyze_one(db: &mut Db, item: i64, opts: &Options) -> CliResult {
    let parts = db.parts_by_item(item)?;
    if !parts.iter().any(|p| is_strm(&p.file)) {
        return Err(exec_original().into()); // local media: keep the official path
    }
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
    let ffmpeg = env_or("ASSISTANT_FFMPEG", "ffmpeg");
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
        }
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

/// Replaces this process with the preserved Plex binary so unhandled
/// modes keep their official behavior. Only returns on failure.
fn exec_original() -> String {
    let argv: Vec<OsString> = std::env::args_os().collect();
    let mut original = argv.first().cloned().unwrap_or_default();
    original.push(".real");
    let original = PathBuf::from(original);
    if !original.exists() {
        return format!("original scanner not found at {}", original.display());
    }
    exec(&original, &argv[1..]).to_string()
}

fn exec(path: &Path, args: &[OsString]) -> std::io::Error {
    Command::new(path).arg0(OsStr::new(path)).args(args).exec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<ScanFlags, String> {
        parse_flags(&args.iter().map(OsString::from).collect::<Vec<_>>())
    }

    #[test]
    fn plex_style_argv() {
        let f = parse(&[
            "--analyze",
            "--log-file-suffix",
            " Analysis",
            "--item",
            "123",
            "--quick",
            "-x",
            "positional",
        ])
        .unwrap();
        assert!(f.analyze);
        assert_eq!((f.item, f.section), (123, 0));
        assert_eq!(f.library_db.to_str(), Some(plexdb::DEFAULT_PATH));
    }

    #[test]
    fn inline_values_and_overrides() {
        let f = parse(&[
            "--analyze=true",
            "--section=4",
            "--library-db=/tmp/lib.db",
            "--ffprobe",
            "/bin/ffprobe",
            "--ua=UA/2",
            "--runtime-sock",
            "",
        ])
        .unwrap();
        assert!(f.analyze);
        assert_eq!(f.section, 4);
        assert_eq!(f.library_db.to_str(), Some("/tmp/lib.db"));
        assert_eq!(f.ffprobe, "/bin/ffprobe");
        assert_eq!(f.ua, "UA/2");
        assert_eq!(f.runtime_sock.to_str(), Some(""));
    }

    #[test]
    fn unknown_flag_does_not_swallow_a_flag() {
        let f = parse(&["--scan", "--item", "7", "--refresh=1", "--section", "2"]).unwrap();
        assert!(!f.analyze);
        assert_eq!((f.item, f.section), (7, 2));
    }

    #[test]
    fn known_flags_take_dash_values() {
        let f = parse(&["--analyze", "--item", "-5", "--ua", "-odd-"]).unwrap();
        assert_eq!(f.item, -5);
        assert_eq!(f.ua, "-odd-");
    }

    #[test]
    fn lone_dash_is_positional() {
        let f = parse(&["-", "--item", "3"]).unwrap();
        assert_eq!(f.item, 3);
    }

    #[test]
    fn stops_at_double_dash() {
        let f = parse(&["--analyze=false", "--", "--item", "9"]).unwrap();
        assert!(!f.analyze);
        assert_eq!(f.item, 0);
    }

    #[test]
    fn errors() {
        assert_eq!(
            parse(&["--item", "abc"]).unwrap_err(),
            "invalid argument \"abc\" for --item"
        );
        assert_eq!(
            parse(&["--item"]).unwrap_err(),
            "flag needs an argument: --item"
        );
        assert!(parse(&["--analyze=maybe"]).is_err());
    }
}

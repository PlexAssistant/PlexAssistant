use std::ffi::{OsStr, OsString};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::CliResult;
use crate::linkres::{self, DEFAULT_UA};
use crate::plexdb::{self, Db};
use crate::runtime::DEFAULT_SOCK;
use crate::scanner::{self, Options, credits, is_strm};

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
            ffprobe: std::env::var("ASSISTANT_FFPROBE")
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "ffprobe".into()),
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
    };
    let mut db = Db::open(&flags.library_db)?;
    // one refresh budget per target for the whole run
    let mut refreshed = std::collections::HashSet::new();
    if flags.item > 0 {
        analyze_one(&mut db, flags.item, &opts, &mut refreshed).await
    } else {
        analyze_section(&mut db, flags.section, &opts, &mut refreshed).await
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

/// Credits detection for one file: find the item, resolve a strm target
/// through the daemon, run the black-window heuristic and write the
/// marker. Local files and unknown paths keep the official path.
async fn run_credits(file: &OsStr) -> CliResult {
    let file_str = file.to_string_lossy().into_owned();
    let opts = Options {
        ffprobe: std::env::var("ASSISTANT_FFPROBE").unwrap_or_else(|_| "ffprobe".into()),
        user_agent: DEFAULT_UA.into(),
        runtime_sock: std::env::var("ASSISTANT_SOCK")
            .ok()
            .map(PathBuf::from)
            .filter(|s| !s.as_os_str().is_empty())
            .or_else(|| Some(PathBuf::from(DEFAULT_SOCK))),
    };

    let mut db = Db::open(plexdb::DEFAULT_PATH)?;
    let Some(item) = db.item_id_by_part_file(&file_str)? else {
        return Err(exec_original().into());
    };
    let target = match linkres::parse_strm(file) {
        Ok(target) if target.is_remote => target,
        _ => return Err(exec_original().into()), // local or unreadable: official path
    };

    let Some(link) = scanner::resolve_link(&target.value, &opts).await else {
        println!("credits: resolve failed for {file_str}");
        return Ok(0); // nothing to detect; PMS counts this a success
    };
    let ffmpeg = std::env::var("ASSISTANT_FFMPEG").unwrap_or_else(|_| "ffmpeg".into());
    match credits::detect_credits(&ffmpeg, &link.url, &opts.user_agent).await {
        Some((start, end)) => {
            db.write_marker(item, plexdb::MARKER_CREDITS, start, end)?;
            println!("credits: marker {start}-{end} ms on item {item}");
        }
        None => println!("credits: no window found for {file_str}"),
    }
    Ok(0)
}

async fn analyze_one(
    db: &mut Db,
    item: i64,
    opts: &Options,
    refreshed: &mut std::collections::HashSet<String>,
) -> CliResult {
    let parts = db.parts_by_item(item)?;
    if !parts.iter().any(|p| is_strm(&p.file)) {
        return Err(exec_original().into()); // local media: keep the official path
    }
    let n = scanner::analyze_item(db, item, opts, refreshed).await?;
    println!("item {item}: analyzed {n} strm part(s)");
    Ok(0)
}

async fn analyze_section(
    db: &mut Db,
    section: i64,
    opts: &Options,
    refreshed: &mut std::collections::HashSet<String>,
) -> CliResult {
    let ids = db.items_by_section(section)?;
    for &id in &ids {
        let parts = db.parts_by_item(id)?;
        if !parts.iter().any(|p| is_strm(&p.file)) {
            continue;
        }
        if let Err(e) = scanner::analyze_item(db, id, opts, refreshed).await {
            println!("item {id}: {e}");
        }
    }
    if let Err(e) = db.checkpoint() {
        println!("checkpoint: {e}");
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

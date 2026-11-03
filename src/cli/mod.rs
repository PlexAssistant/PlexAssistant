//! Wires the assistant command tree.
//!
//! The binary is multi-call: when Plex invokes it through the install-time
//! symlinks (Scanner / Transcoder), the subcommand is derived from argv[0];
//! manual invocations use subcommands directly.

mod install;
mod runtime;
mod scanner;
mod transcoder;

use std::error::Error;
use std::ffi::OsString;
use std::path::Path;

use clap::{CommandFactory, Parser, Subcommand};

/// Overridden at build time through the ASSISTANT_VERSION env var.
pub const VERSION: &str = match option_env!("ASSISTANT_VERSION") {
    Some(v) => v,
    None => "dev",
};

type CliResult = Result<i32, Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(name = "assistant", version = VERSION, about = "Native STRM support for Plex")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Scanner(scanner::Args),
    Transcoder(transcoder::Args),
    Runtime(runtime::Args),
    Install(install::Args),
    Intro(IntroArgs),
    /// Prints the inputs and outputs of the credits-detection model.
    CreditsProbe(CreditsProbeArgs),
}

/// Prints the credits model's inputs and outputs. Detection does not
/// use the result yet; this only checks that the graph loads.
#[derive(Debug, clap::Args)]
struct CreditsProbeArgs {
    /// model file; defaults to the copy under the Plex config root
    #[arg(long)]
    model: Option<std::path::PathBuf>,
    /// Plex config root, used when --model is omitted
    #[arg(long, default_value = crate::scanner::credits_assets::DEFAULT_CONFIG)]
    config: std::path::PathBuf,
}

/// Detects intro markers by cross-comparing audio fingerprints of
/// episodes within each season.
#[derive(Debug, clap::Args)]
struct IntroArgs {
    /// library section id
    #[arg(long)]
    section: i64,
    /// library database path
    #[arg(long, default_value = crate::plexdb::DEFAULT_PATH)]
    library_db: std::path::PathBuf,
    #[arg(long, default_value = "ffmpeg")]
    ffmpeg: String,
    #[arg(long, default_value = "fpcalc")]
    fpcalc: String,
    #[arg(long, default_value = "ffprobe")]
    ffprobe: String,
    #[arg(long, default_value = crate::linkres::DEFAULT_UA)]
    ua: String,
    #[arg(long)]
    runtime_sock: Option<std::path::PathBuf>,
}

/// Maps argv[0] to a subcommand by basename. Exact symlink names are
/// confirmed against a real Plex install.
fn dispatch_argv0(argv0: &Path) -> Option<&'static str> {
    let base = argv0.file_name()?.to_string_lossy().to_lowercase();
    if base.contains("scanner") {
        Some("scanner")
    } else if base.contains("transcoder") {
        Some("transcoder")
    } else {
        None
    }
}

/// Reports whether the first argument already names a subcommand.
fn has_command(args: &[OsString]) -> bool {
    let Some(first) = args.first() else {
        return false;
    };
    Cli::command()
        .get_subcommands()
        .any(|c| first == c.get_name())
}

/// Resolves the invocation mode, runs the matching command and returns
/// the process exit code.
pub async fn run() -> i32 {
    let mut argv: Vec<OsString> = std::env::args_os().collect();
    let argv0 = argv.first().cloned().unwrap_or_default();
    if let Some(sub) = dispatch_argv0(Path::new(&argv0))
        && !has_command(&argv[1..])
    {
        argv.insert(1, sub.into());
    }

    let cli = match Cli::try_parse_from(&argv) {
        Ok(cli) => cli,
        Err(e) => e.exit(),
    };
    let result = match cli.command {
        Command::Scanner(args) => scanner::run(args).await,
        Command::Transcoder(args) => transcoder::run(args, Path::new(&argv0)).await,
        Command::Runtime(args) => runtime::run(args).await,
        Command::Install(args) => install::run(args),
        Command::Intro(args) => run_intro(args).await,
        Command::CreditsProbe(args) => run_credits_probe(args),
    };
    result.unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        1
    })
}

/// Loads the credits model and prints its signature.
fn run_credits_probe(args: CreditsProbeArgs) -> CliResult {
    let path = args
        .model
        .unwrap_or_else(|| crate::scanner::credits_assets::path(&args.config));
    let signature = crate::scanner::credits_net::signature(&path)?;
    println!("{signature}");
    Ok(0)
}

/// Runs intro detection for a library section.
async fn run_intro(args: IntroArgs) -> CliResult {
    let mut db = crate::plexdb::Db::open(&args.library_db)?;
    let sock = args.runtime_sock;
    let n = crate::scanner::intro::detect_section_intros(
        &mut db,
        args.section,
        &args.ffmpeg,
        &args.fpcalc,
        &args.ffprobe,
        &args.ua,
        sock.as_ref(),
    )
    .await?;
    println!("intro detection: {n} marker(s) written");
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch() {
        for (argv0, want) in [
            (
                "/config/Library/Application Support/Plex Media Server/Scanners/Movies/Plex Movie Scanner",
                Some("scanner"),
            ),
            (
                "/usr/lib/plexmediaserver/Plex Media Scanner",
                Some("scanner"),
            ),
            (
                "/usr/lib/plexmediaserver/Plex Transcoder",
                Some("transcoder"),
            ),
            (
                "/usr/lib/plexmediaserver/Plex Transcoder.native",
                Some("transcoder"),
            ),
            ("assistant", None),
            ("/usr/local/bin/assistant", None),
            ("", None),
        ] {
            assert_eq!(dispatch_argv0(Path::new(argv0)), want, "{argv0}");
        }
    }

    #[test]
    fn known_commands() {
        for name in [
            "scanner",
            "transcoder",
            "runtime",
            "install",
            "intro",
            "credits-probe",
        ] {
            assert!(has_command(&[name.into()]), "{name}");
        }
        assert!(!has_command(&["nope".into()]));
        assert!(!has_command(&[]));
    }

    #[test]
    fn command_tree_is_valid() {
        Cli::command().debug_assert();
    }
}

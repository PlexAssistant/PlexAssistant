//! Wires the assistant command tree.
//!
//! The binary is multi-call: when Plex invokes it through the install-time
//! symlinks (Scanner / Transcoder), the subcommand is derived from `argv[0]`;
//! manual invocations use subcommands directly.

mod credits_probe;
mod install;
mod intro;
mod marker;
mod runtime;
mod scanner;
mod tracks;
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
    Intro(intro::Args),
    CreditsProbe(credits_probe::Args),
    /// Shows and edits the markers detection booked.
    Marker(marker::Args),
    /// Shows and restores remembered audio and subtitle choices.
    Tracks(tracks::Args),
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
        Command::Intro(args) => intro::run(args).await,
        Command::CreditsProbe(args) => credits_probe::run(args),
        Command::Marker(args) => marker::run(args),
        Command::Tracks(args) => tracks::run(args),
    };
    result.unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        1
    })
}

/// The ffprobe and ffmpeg the image installs. A bare name would find
/// whichever one is first on `PATH`, which outside the image is not the
/// one the fixtures were captured with. `ASSISTANT_FFPROBE` and
/// `ASSISTANT_FFMPEG` still override them.
const DEFAULT_FFPROBE: &str = "/usr/local/bin/ffprobe";
const DEFAULT_FFMPEG: &str = "/usr/local/bin/ffmpeg";

/// `name` from the environment when set and not empty, else `default`.
/// Plex starts the scanner and transcoder with its own argv, so their
/// tool paths and sockets can only be overridden this way.
fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_owned())
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
            "marker",
            "tracks",
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

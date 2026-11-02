//! Wires the assistant command tree.
//!
//! The binary is multi-call: when Plex invokes it through the install-time
//! symlinks (Scanner / Transcoder), the subcommand is derived from argv[0];
//! manual invocations use subcommands directly.

mod install;
mod runtime;
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
    Transcoder(transcoder::Args),
    Runtime(runtime::Args),
    Install(install::Args),
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
        Command::Transcoder(args) => transcoder::run(args, Path::new(&argv0)).await,
        Command::Runtime(args) => runtime::run(args).await,
        Command::Install(args) => install::run(args),
    };
    result.unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        1
    })
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
        for name in ["transcoder", "runtime", "install"] {
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

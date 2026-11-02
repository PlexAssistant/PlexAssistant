use std::path::PathBuf;

use clap::Subcommand;

use super::{CliResult, VERSION};
use crate::install::{DEFAULT_SUPPORT_DIR, Installer, TARGETS};

/// Install scanner/transcoder symlinks into Plex.
#[derive(Debug, clap::Args)]
pub struct Args {
    #[command(flatten)]
    dirs: Dirs,
    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(Debug, Subcommand)]
enum Action {
    /// Verify installation integrity
    Verify {
        #[command(flatten)]
        dirs: Dirs,
    },
    /// Restore original Plex binaries
    Revert {
        #[command(flatten)]
        dirs: Dirs,
    },
}

#[derive(Debug, Clone, clap::Args)]
struct Dirs {
    /// Plex binary directory
    #[arg(long = "plex-dir", default_value = "/usr/lib/plexmediaserver")]
    plex_dir: PathBuf,
    /// Plex data directory
    #[arg(long = "support-dir", default_value = DEFAULT_SUPPORT_DIR)]
    support_dir: PathBuf,
}

impl Dirs {
    fn installer(self) -> std::io::Result<Installer> {
        Ok(Installer {
            plex_dir: self.plex_dir,
            support_dir: self.support_dir,
            bin_path: std::env::current_exe()?,
            version: VERSION.into(),
        })
    }
}

pub fn run(args: Args) -> CliResult {
    match args.action {
        None => {
            let inst = args.dirs.installer()?;
            inst.install()?;
            println!(
                "installed [{}] into {}",
                TARGETS.join(" "),
                inst.plex_dir.display()
            );
        }
        Some(Action::Verify { dirs }) => {
            let mut failed = false;
            for check in dirs.installer()?.verify() {
                let status = match &check.problem {
                    None => "ok".to_owned(),
                    Some(problem) => {
                        failed = true;
                        format!("FAIL: {problem}")
                    }
                };
                println!("{:<20} {status}", check.target);
            }
            if failed {
                return Err("verification failed".into());
            }
        }
        Some(Action::Revert { dirs }) => {
            dirs.installer()?.revert()?;
            println!("originals restored");
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command};
    use clap::Parser;

    fn parse(argv: &[&str]) -> Args {
        let Command::Install(args) = Cli::try_parse_from(argv).unwrap().command else {
            panic!("not install");
        };
        args
    }

    #[test]
    fn subcommands_take_the_dirs() {
        let args = parse(&["assistant", "install", "--plex-dir", "/p"]);
        assert!(args.action.is_none());
        assert_eq!(args.dirs.plex_dir.to_str(), Some("/p"));
        assert_eq!(args.dirs.support_dir.to_str(), Some(DEFAULT_SUPPORT_DIR));

        let args = parse(&["assistant", "install", "revert", "--plex-dir", "/q"]);
        let Some(Action::Revert { dirs }) = args.action else {
            panic!("not revert");
        };
        assert_eq!(dirs.plex_dir.to_str(), Some("/q"));

        let args = parse(&["assistant", "install", "verify"]);
        assert!(matches!(args.action, Some(Action::Verify { .. })));
    }
}

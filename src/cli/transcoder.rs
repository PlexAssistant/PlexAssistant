use std::ffi::OsString;
use std::path::Path;

use super::{CliResult, env_or};
use crate::linkres::DEFAULT_UA;
use crate::runtime::DEFAULT_SOCK;
use crate::transcoder::{self, DaemonResolver};

/// Run as the Plex Transcoder wrapper: "-i" strm inputs are swapped for
/// cached direct links, then the preserved original runs as a child.
/// Nothing is parsed: Plex passes raw ffmpeg-style argv.
#[derive(Debug, clap::Args)]
#[command(disable_help_flag = true)]
pub struct Args {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
    args: Vec<OsString>,
}

pub async fn run(args: Args, argv0: &Path) -> CliResult {
    let resolver = DaemonResolver {
        sock: Some(env_or("ASSISTANT_SOCK", DEFAULT_SOCK).into()),
        user_agent: DEFAULT_UA.into(),
    };
    let mut real = argv0.as_os_str().to_owned();
    real.push(".real");
    Ok(transcoder::run(&args.args, Path::new(&real), &resolver, DEFAULT_UA).await)
}

#[cfg(test)]
mod tests {
    use crate::cli::{Cli, Command};
    use clap::Parser;
    use std::ffi::OsString;

    #[test]
    fn passes_raw_argv_through() {
        let raw = [
            "-codec:0",
            "h264",
            "-i",
            "/x/a.strm",
            "--help",
            "-y",
            "transcoder",
            "-",
        ];
        let argv = ["Plex Transcoder", "transcoder"].iter().chain(&raw);
        let Command::Transcoder(args) = Cli::try_parse_from(argv).unwrap().command else {
            panic!("not the transcoder");
        };
        assert_eq!(args.args, raw.map(OsString::from));
    }
}

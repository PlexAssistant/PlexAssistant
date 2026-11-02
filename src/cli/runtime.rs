use std::path::PathBuf;
use std::sync::Arc;

use super::CliResult;
use crate::linkres::DEFAULT_UA;
use crate::runtime::{Cache, DEFAULT_SOCK, Daemon};

/// Run the resident link-cache daemon. Links are stored persistently and
/// trusted until playback reports them dead.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// unix socket path
    #[arg(long, default_value = DEFAULT_SOCK)]
    sock: PathBuf,
    /// cache database path
    #[arg(long, default_value = "/config/assistant/cache.db")]
    cache: PathBuf,
    /// user agent for cloud drives
    #[arg(long, default_value = DEFAULT_UA)]
    ua: String,
    /// scan resolutions per upstream domain per hour
    #[arg(long = "quota-per-hour", default_value_t = 30)]
    quota_per_hour: usize,
}

pub async fn run(args: Args) -> CliResult {
    let cache = Cache::open(&args.cache)?;
    println!(
        "runtime: listening on {}, cache {}, quota {}/h",
        args.sock.display(),
        args.cache.display(),
        args.quota_per_hour
    );
    Daemon::new(Arc::new(cache), args.ua, args.quota_per_hour)
        .serve(&args.sock)
        .await?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use crate::cli::{Cli, Command};
    use clap::Parser;

    #[test]
    fn flags() {
        let cli = Cli::try_parse_from([
            "assistant",
            "runtime",
            "--sock",
            "/run/assistant/assistant.sock",
            "--quota-per-hour",
            "5",
        ])
        .unwrap();
        let Command::Runtime(args) = cli.command else {
            panic!("not runtime");
        };
        assert_eq!(args.quota_per_hour, 5);
        assert_eq!(args.cache.to_str(), Some("/config/assistant/cache.db"));
        assert_eq!(args.ua, crate::linkres::DEFAULT_UA);
    }
}

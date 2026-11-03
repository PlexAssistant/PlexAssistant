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
    /// library database, read when a detail page is opened
    #[arg(long, default_value = crate::plexdb::DEFAULT_PATH)]
    library_db: PathBuf,
    /// ffprobe used when a detail page needs analysis
    #[arg(long, default_value = "ffprobe")]
    ffprobe: String,
}

pub async fn run(args: Args) -> CliResult {
    let cache = Arc::new(Cache::open(&args.cache)?);
    println!(
        "runtime: listening on {}, cache {}, quota {}/h",
        args.sock.display(),
        args.cache.display(),
        args.quota_per_hour
    );
    let watch = watch_details(
        args.library_db.clone(),
        args.cache.clone(),
        args.ffprobe.clone(),
        args.ua.clone(),
        args.sock.clone(),
    );
    let daemon = Daemon::new(Arc::clone(&cache), args.ua, args.quota_per_hour);
    let serve = daemon.serve(&args.sock);
    tokio::pin!(watch);
    tokio::pin!(serve);
    tokio::select! {
        result = &mut serve => result?,
        result = &mut watch => result?,
    }
    Ok(0)
}

/// Analyses a strm item when its detail page shows up in the Plex log.
///
/// An item that already has media info is left alone. A missing log or a
/// library that cannot be opened is not fatal: the link cache keeps
/// serving.
async fn watch_details(
    library: PathBuf,
    cache: PathBuf,
    ffprobe: String,
    ua: String,
    sock: PathBuf,
) -> std::io::Result<()> {
    let log = crate::runtime::log_path();
    crate::runtime::follow(&log, move |item| {
        let library = library.clone();
        let cache = cache.clone();
        let ffprobe = ffprobe.clone();
        let ua = ua.clone();
        let sock = sock.clone();
        tokio::spawn(async move {
            if let Err(e) = fill_item(&library, &cache, &ffprobe, &ua, &sock, item).await {
                println!("detail {item}: {e}");
            }
        });
    })
    .await
}

async fn fill_item(
    library: &std::path::Path,
    cache: &std::path::Path,
    ffprobe: &str,
    ua: &str,
    sock: &std::path::Path,
    item: i64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut db = crate::plexdb::Db::open(library)?;
    let parts = db.parts_by_item(item)?;
    let pending = parts
        .iter()
        .any(|part| crate::scanner::is_strm(&part.file) && (part.duration <= 0 || part.size <= 0));
    if !pending {
        return Ok(());
    }
    let opts = crate::scanner::Options {
        ffprobe: ffprobe.to_owned(),
        user_agent: ua.to_owned(),
        runtime_sock: Some(sock.to_owned()).filter(|p| !p.as_os_str().is_empty()),
        cache: Some(cache.to_owned()),
    };
    let n = crate::scanner::analyze_item(&mut db, item, &opts, &mut Default::default()).await?;
    if n > 0 {
        println!("detail {item}: analyzed {n} strm part(s)");
    }
    Ok(())
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
        assert_eq!(args.ffprobe, "ffprobe");
    }
}

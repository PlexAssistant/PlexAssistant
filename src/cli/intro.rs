use std::path::PathBuf;

use super::CliResult;
use crate::plexdb::{self, Db};

/// Detects intro markers by cross-comparing audio fingerprints of
/// episodes within each season.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// library section id
    #[arg(long)]
    section: i64,
    /// library database path
    #[arg(long, default_value = plexdb::DEFAULT_PATH)]
    library_db: PathBuf,
    #[arg(long, default_value = "ffmpeg")]
    ffmpeg: String,
    #[arg(long, default_value = "fpcalc")]
    fpcalc: String,
    #[arg(long, default_value = "ffprobe")]
    ffprobe: String,
    #[arg(long, default_value = crate::linkres::DEFAULT_UA)]
    ua: String,
    #[arg(long)]
    runtime_sock: Option<PathBuf>,
}

/// Runs intro detection for a library section.
pub async fn run(args: Args) -> CliResult {
    let mut db = Db::open(&args.library_db)?;
    let n = crate::scanner::intro::detect_section_intros(
        &mut db,
        args.section,
        &args.ffmpeg,
        &args.fpcalc,
        &args.ffprobe,
        &args.ua,
        args.runtime_sock.as_ref(),
    )
    .await?;
    println!("intro detection: {n} marker(s) written");
    Ok(0)
}

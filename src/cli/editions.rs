//! Writes movie edition titles over the Plex HTTP API.
//!
//! The logic lives in the plex-editions crate — the PlexEditions project,
//! which also ships its own standalone image for servers without the
//! assistant. This front end needs no configuration inside the container:
//! the URL defaults to this Plex and the token to its `Preferences.xml`.
//! `PLEX_URL`, `PLEX_TOKEN`, `PLEX_CONFIG_DIR` and the `EDITION_*`
//! variables override, as the PlexEditions README documents.

use clap::Subcommand;
use plex_editions::api;
use plex_editions::config::Config;
use plex_editions::error::Error;
use plex_editions::run::{self, Mode};
use plex_editions::startup;
use plex_editions::ws;
use tracing::info;
use tracing_subscriber::EnvFilter;

use super::CliResult;

/// This Plex: the assistant lives in its container.
const DEFAULT_PLEX_URL: &str = "http://127.0.0.1:32400";

/// Writes movie edition titles on this server.
#[derive(Debug, clap::Args)]
pub struct Args {
    #[command(subcommand)]
    action: Action,
}

#[derive(Debug, Subcommand)]
enum Action {
    /// Writes editions for movies that do not have one, then exits.
    All(Run),
    /// Listens for new movies and writes an edition for each.
    New(Run),
    /// Clears editions in movie libraries, then exits.
    Reset(Run),
}

#[derive(Debug, clap::Args)]
struct Run {
    /// Print the edition that would be written. Do not change the server.
    #[arg(long)]
    dry_run: bool,
}

pub async fn run(args: Args) -> CliResult {
    // The library logs through tracing; give the lines a subscriber so
    // they reach stdout. RUST_LOG tunes it, `info` by default.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .without_time()
        .init();
    match args.action {
        Action::All(run) => one_shot(Mode::All, run.dry_run).await?,
        Action::Reset(run) => one_shot(Mode::Reset, run.dry_run).await?,
        Action::New(run) => listen(run.dry_run).await?,
    }
    Ok(0)
}

/// Connect once, process every movie library, report the counts.
async fn one_shot(mode: Mode, dry_run: bool) -> Result<(), Error> {
    let mut config = Config::from_env_with_url(DEFAULT_PLEX_URL)?;
    config.dry_run = dry_run;
    let client = api::Client::new(&config.plex_url, &config.plex_token)?;
    let name = client.friendly_name().await?;
    info!("connected to {name}");
    let stats = run::run(&config, &client, mode).await?;
    match mode {
        Mode::All => info!(
            written = stats.written,
            skipped = stats.skipped,
            failed = stats.failed,
            "done"
        ),
        Mode::Reset => info!(
            cleared = stats.written,
            skipped = stats.skipped,
            failed = stats.failed,
            "done"
        ),
    }
    Ok(())
}

/// Wait for the server, then listen until the process is stopped.
async fn listen(dry_run: bool) -> Result<(), Error> {
    let (mut config, client, name) = startup::await_ready(DEFAULT_PLEX_URL).await?;
    config.dry_run = dry_run;
    info!("connected to {name}");
    // The listener writes editions for movies that do not have one
    // before each connection, so a restart also covers the backlog.
    ws::listen(&config, &client).await
}

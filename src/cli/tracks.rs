//! Shows and restores the audio and subtitle choices a rewrite remembered.
//!
//! A scan that rewrites a part keeps the previous stream ids. Restoring
//! puts those ids back on the part and forgets the remembered choice, so
//! the next episode is not filled from it.

use std::path::PathBuf;

use clap::Subcommand;

use super::CliResult;
use crate::plexdb::Db;
use crate::runtime::Cache;

/// Shows and restores remembered audio and subtitle choices.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// ledger database; the assistant's cache
    #[arg(long, default_value = "/config/assistant/cache.db")]
    cache: PathBuf,
    /// library database the previous stream ids are written back to
    #[arg(long, default_value = crate::plexdb::DEFAULT_PATH)]
    library_db: PathBuf,
    #[command(subcommand)]
    action: Action,
}

#[derive(Debug, Subcommand)]
enum Action {
    /// Prints the choice remembered for one account on one show.
    Show {
        #[arg(long)]
        account: i64,
        #[arg(long)]
        show: i64,
    },
    /// Puts the pre-rewrite stream ids back and forgets the choice.
    ///
    /// `part` is the part those ids belonged to. Without it, only the
    /// remembered choice is forgotten.
    Restore {
        #[arg(long)]
        account: i64,
        #[arg(long)]
        show: i64,
        /// part the previous stream ids are written back onto
        #[arg(long)]
        part: Option<i64>,
    },
}

pub fn run(args: Args) -> CliResult {
    let cache = Cache::open(&args.cache)?;
    match args.action {
        Action::Show { account, show } => print_choice(&cache, account, show),
        Action::Restore {
            account,
            show,
            part,
        } => restore(&cache, &args.library_db, account, show, part),
    }
}

fn print_choice(cache: &Cache, account: i64, show: i64) -> CliResult {
    let Some(row) = cache.track_choice(account, show)? else {
        println!("account {account} show {show}: nothing remembered");
        return Ok(0);
    };
    println!(
        "account {account} show {show}: audio {} subtitle {} previous audio {:?} subtitle {:?}",
        side(&row.audio),
        side(&row.subtitle),
        row.previous_audio,
        row.previous_subtitle,
    );
    Ok(0)
}

fn side(choice: &Option<crate::plexdb::TrackChoice>) -> String {
    match choice {
        Some(choice) => format!("{}#{}", choice.language, choice.index),
        None => "-".into(),
    }
}

fn restore(
    cache: &Cache,
    library: &std::path::Path,
    account: i64,
    show: i64,
    part: Option<i64>,
) -> CliResult {
    let Some(row) = cache.clear_track_choice(account, show)? else {
        println!("account {account} show {show}: nothing remembered");
        return Ok(0);
    };
    let Some(part) = part else {
        println!("account {account} show {show}: forgotten");
        return Ok(0);
    };
    let db = Db::open(library)?;
    db.retarget_selection(account, part, row.previous_audio, row.previous_subtitle)?;
    println!(
        "account {account} show {show}: part {part} restored audio {:?} subtitle {:?}",
        row.previous_audio, row.previous_subtitle,
    );
    Ok(0)
}

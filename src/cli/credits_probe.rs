use std::path::PathBuf;

use super::CliResult;
use crate::scanner::credits::{assets, net};

/// Prints the inputs and outputs of the credits-detection model. This
/// only checks that the graph can be loaded and run.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// model file; defaults to the copy under the Plex config root
    #[arg(long)]
    model: Option<PathBuf>,
    /// Plex config root, used when --model is omitted
    #[arg(long, default_value = assets::DEFAULT_CONFIG)]
    config: PathBuf,
}

/// Loads the credits model and prints its signature.
pub fn run(args: Args) -> CliResult {
    let path = args.model.unwrap_or_else(|| assets::path(&args.config));
    let plan = net::load(&path)?;
    println!("{}", plan.signature());
    Ok(0)
}

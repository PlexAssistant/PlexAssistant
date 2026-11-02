//! Command assistant is the single binary that replaces the Plex Scanner
//! and Transcoder inside the container to provide native STRM support.

#[tokio::main]
async fn main() {
    std::process::exit(plex_assistant::cli::run().await);
}

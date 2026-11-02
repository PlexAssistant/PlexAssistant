//! Command fakecloud is a fake cloud drive for the test rig: /file/{id}
//! resolves to a signed direct link via 302, /dl/{id} serves the bytes
//! with injectable failure modes (403/410/throttle) controlled at runtime
//! via POST /ctl.

mod cloud;
mod duration;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Fake cloud drive for the PlexAssistant test rig")]
struct Args {
    /// listen address; ":8399" listens on every interface
    #[arg(long, default_value = ":8399")]
    addr: String,
    /// media root
    #[arg(long, default_value = "testdata/media")]
    root: PathBuf,
    /// token signing secret
    #[arg(long, default_value = "testrig")]
    secret: String,
    /// default direct-link TTL, Go duration syntax ("1h", "10s")
    #[arg(long, default_value = "1h", value_parser = duration::parse, allow_hyphen_values = true)]
    ttl: i64,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    let addr = match args.addr.strip_prefix(':') {
        Some(port) => format!("0.0.0.0:{port}"),
        None => args.addr,
    };
    println!(
        "fakecloud listening on {addr}, root {}",
        args.root.display()
    );
    let app = cloud::Server::new(&args.secret, args.root, args.ttl).router();
    let served = match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => {
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown_signal())
                .await
        }
        Err(e) => Err(e),
    };
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("fakecloud: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Resolves on SIGTERM or SIGINT. As a container's PID 1 the process gets
/// no default signal handling, so `docker stop` would otherwise wait out
/// its timeout and SIGKILL us.
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}

//! The resident daemon: serves the link cache over a unix socket.

mod handlers;
#[cfg(test)]
mod tests;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use std::{fs, io};

use axum::Router;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use tokio::net::UnixListener;

use super::api::Resolved;
use super::breaker::Breakers;
use super::cache::{Cache, Entry};
use super::quota::Quota;
use crate::linkres;
use handlers::{error, host_of, json};

/// The daemon socket, created by the s6 service with the right ownership.
pub const DEFAULT_SOCK: &str = "/run/assistant/assistant.sock";

const QUOTA_WINDOW: Duration = Duration::from_secs(60 * 60);

/// Serves the link cache over a unix socket. Semantics: a link is
/// resolved once and then trusted forever; refresh happens only when a
/// caller reports it dead while playing. Scan-driven resolutions pass a
/// breaker, a dead-link ledger and a per-domain quota; playback (refresh)
/// bypasses all three but its outcome still feeds them.
pub struct Daemon {
    cache: Arc<Cache>,
    ua: String,
    breakers: Breakers,
    quota: Quota,
}

impl Daemon {
    /// Wires a daemon over cache, resolving with ua and a scan-resolution
    /// quota of quota_per_hour per upstream domain.
    pub fn new(cache: Arc<Cache>, ua: impl Into<String>, quota_per_hour: usize) -> Arc<Self> {
        Arc::new(Self {
            cache,
            ua: ua.into(),
            breakers: Breakers::default(),
            quota: Quota::new(quota_per_hour, QUOTA_WINDOW),
        })
    }

    /// The HTTP API; also usable over TCP for tests.
    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/resolve", post(handlers::handle_resolve))
            .route("/refresh", post(handlers::handle_refresh))
            .route("/dead", post(handlers::handle_dead))
            .route("/state", get(handlers::handle_state))
            .with_state(Arc::clone(self))
    }

    /// Listens on the unix socket, replacing a stale one, and serves until
    /// the listener fails.
    pub async fn serve(self: &Arc<Self>, sock: impl AsRef<Path>) -> io::Result<()> {
        let sock = sock.as_ref();
        match fs::remove_file(sock) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        let listener = UnixListener::bind(sock)?;
        // owner+group only, whatever the service umask happens to be
        fs::set_permissions(sock, fs::Permissions::from_mode(0o660))?;
        axum::serve(listener, self.router()).await
    }

    async fn resolve_and_store(
        &self,
        target: &str,
        force: bool,
        offset: i64,
        playback: bool,
    ) -> Response {
        let domain = host_of(target);
        let link = match linkres::resolve(target, &self.ua).await {
            Ok(link) => link,
            Err(err) => {
                self.breakers.record_failure(&domain, Instant::now());
                let booked = self
                    .cache
                    .record_dead(target, err.is_permanent(), SystemTime::now());
                if let Err(e) = booked {
                    return error(StatusCode::INTERNAL_SERVER_ERROR, e);
                }
                return error(
                    StatusCode::BAD_GATEWAY,
                    crate::scanner::redact_urls(&err.to_string()),
                );
            }
        };
        self.breakers.record_success(&domain);
        // A scan probe reads one byte at the start. Playback reads the file:
        // the size the headers reported, and a seek when it opened past the
        // start. The quota still counts requests only. The failure row, the
        // usage row and the link commit together.
        let (bytes, seek) = if playback {
            (link.size, offset > 0)
        } else {
            (1, false)
        };
        let entry = Entry {
            target: target.to_owned(),
            direct: link.url.clone(),
            ua: self.ua.clone(),
            expires_at: link.expires_at,
            size: link.size,
            ..Default::default()
        };
        if let Err(e) = self
            .cache
            .store_resolved(&entry, &domain, bytes, seek, SystemTime::now())
        {
            return error(StatusCode::INTERNAL_SERVER_ERROR, e);
        }
        json(&Resolved {
            direct: link.url,
            expires_at: link.expires_at,
            cached: force,
        })
    }
}

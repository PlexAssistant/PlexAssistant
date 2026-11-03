use std::path::Path;
use std::time::Duration;

use reqwest::header::CONTENT_TYPE;

use super::daemon::{Resolved, TargetRequest};

const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("daemon {method}: status {status}")]
    Status { method: &'static str, status: u16 },
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error("daemon answer: {0}")]
    Decode(#[from] serde_json::Error),
}

/// Talks to the daemon over its unix socket. Callers fall back to
/// resolving directly when the daemon is not running.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
}

impl Client {
    pub fn new(sock: impl AsRef<Path>) -> Result<Self, ClientError> {
        let http = reqwest::Client::builder()
            .unix_socket(sock.as_ref())
            .timeout(TIMEOUT)
            .build()?;
        Ok(Self { http })
    }

    /// Returns the cached or freshly resolved link for a scan.
    pub async fn resolve(&self, target: &str) -> Result<Resolved, ClientError> {
        self.post("resolve", target, 0, false).await
    }

    /// Returns the link for playback and records `seek` when playback
    /// opened past the start.
    pub async fn resolve_at(&self, target: &str, seek: bool) -> Result<Resolved, ClientError> {
        self.post("resolve", target, i64::from(seek), true).await
    }

    /// Forces a re-resolve for a scan whose cached link was dead.
    pub async fn refresh(&self, target: &str) -> Result<Resolved, ClientError> {
        self.post("refresh", target, 0, false).await
    }

    /// Forces a re-resolve for playback. `seek` records that playback
    /// opened past the start.
    pub async fn refresh_at(&self, target: &str, seek: bool) -> Result<Resolved, ClientError> {
        self.post("refresh", target, i64::from(seek), true).await
    }

    /// Records a dead target in the ledger. `permanent` marks a failure
    /// that will not clear on its own; anything else takes the backoff.
    pub async fn dead(&self, target: &str, permanent: bool) -> Result<(), ClientError> {
        let body = serde_json::to_vec(&TargetRequest {
            target: target.to_owned(),
            permanent,
            ..Default::default()
        })?;
        let resp = self
            .http
            .post("http://assistant/dead")
            .header(CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(ClientError::Status {
                method: "dead",
                status: resp.status().as_u16(),
            });
        }
        Ok(())
    }

    async fn post(
        &self,
        method: &'static str,
        target: &str,
        offset: i64,
        playback: bool,
    ) -> Result<Resolved, ClientError> {
        let body = serde_json::to_vec(&TargetRequest {
            target: target.to_owned(),
            offset,
            playback,
            ..Default::default()
        })?;
        let resp = self
            .http
            .post(format!("http://assistant/{method}"))
            .header(CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(ClientError::Status {
                method,
                status: resp.status().as_u16(),
            });
        }
        Ok(serde_json::from_slice(&resp.bytes().await?)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::cache::tests::test_cache;
    use crate::runtime::{Cache, Daemon, Entry};
    use std::path::PathBuf;
    use std::sync::Arc;

    /// Starts a daemon on a socket inside the cache's temp dir.
    async fn daemon_with(entries: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
        let (dir, cache) = test_cache();
        for (target, direct) in entries {
            cache
                .put(&Entry {
                    target: (*target).into(),
                    direct: (*direct).into(),
                    ..Default::default()
                })
                .unwrap();
        }
        let sock = dir.path().join("assistant.sock");
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();
        let daemon = Daemon::new(Arc::new(cache), "", 30);
        tokio::spawn(async move { axum::serve(listener, daemon.router()).await.unwrap() });
        (dir, sock)
    }

    #[tokio::test]
    async fn resolve_from_cache() {
        let (_dir, sock) = daemon_with(&[("http://x/file/ep1", "http://x/dl/ep1")]).await;
        let got = Client::new(&sock)
            .unwrap()
            .resolve("http://x/file/ep1")
            .await
            .unwrap();
        assert_eq!(
            got,
            Resolved {
                direct: "http://x/dl/ep1".into(),
                expires_at: 0,
                cached: true,
            }
        );
    }

    /// A scan's cache hit reads nothing new; only playback books a read.
    #[tokio::test]
    async fn scan_resolve_is_not_a_playback_read() {
        let (dir, sock) = daemon_with(&[("http://x/file/ep1", "http://x/dl/ep1")]).await;
        let client = Client::new(&sock).unwrap();
        let reads = || {
            let cache = Cache::open(dir.path().join("assistant/cache.db")).unwrap();
            let n: i64 = cache
                .conn()
                .query_row("SELECT IFNULL(sum(reads), 0) FROM cloud_usage", [], |r| {
                    r.get(0)
                })
                .unwrap();
            n
        };

        client.resolve("http://x/file/ep1").await.unwrap();
        assert_eq!(reads(), 0);
        client.resolve_at("http://x/file/ep1", false).await.unwrap();
        assert_eq!(reads(), 1);
    }

    #[tokio::test]
    async fn daemon_errors_surface_as_status() {
        let (_dir, sock) = daemon_with(&[]).await;
        // nothing listens on port 1: the daemon answers 502
        let err = Client::new(&sock)
            .unwrap()
            .refresh("http://127.0.0.1:1/file/ep1")
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "daemon refresh: status 502");
    }

    #[tokio::test]
    async fn missing_socket_is_a_transport_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = Client::new(dir.path().join("none.sock"))
            .unwrap()
            .resolve("http://x/file/ep1")
            .await
            .unwrap_err();
        assert!(matches!(err, ClientError::Http(_)), "{err:?}");
    }
}

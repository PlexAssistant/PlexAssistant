use std::collections::BTreeMap;
use std::fmt::Display;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use std::{fs, io};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use tokio::net::UnixListener;
use url::Url;

use super::breaker::{Admit, Breakers};
use super::cache::{Cache, Entry};
use super::quota::Quota;
use crate::linkres;

/// The daemon socket, created by the s6 service with the right ownership.
pub const DEFAULT_SOCK: &str = "/run/assistant/assistant.sock";

const QUOTA_WINDOW: Duration = Duration::from_secs(60 * 60);

/// Body of /resolve, /refresh and /dead.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TargetRequest {
    #[serde(default)]
    pub target: String,
    /// Set by /dead when the failure will not clear on its own. Absent
    /// on older callers, which book the temporary backoff.
    #[serde(default)]
    pub permanent: bool,
    /// Byte the caller opened the file at. Absent means the start, which
    /// is where a scan probe reads. A later byte is a seek.
    #[serde(default)]
    pub offset: i64,
    /// Set by playback. A scan probe leaves it unset, so a cache hit on
    /// that path is not counted as another read.
    #[serde(default)]
    pub playback: bool,
}

/// The daemon's answer to /resolve and /refresh.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolved {
    pub direct: String,
    pub expires_at: i64,
    pub cached: bool,
}

/// The /state report; fields in key order, matching Go's sorted maps.
#[derive(Serialize)]
struct StateReport {
    breakers: BTreeMap<String, &'static str>,
    entries: i64,
    failures: i64,
    hits: i64,
    quota: BTreeMap<String, usize>,
}

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
            .route("/resolve", post(handle_resolve))
            .route("/refresh", post(handle_refresh))
            .route("/dead", post(handle_dead))
            .route("/state", get(handle_state))
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
                let booked = if err.is_permanent() {
                    self.cache.record_permanent(target, SystemTime::now())
                } else {
                    self.cache.record_failure(target, SystemTime::now())
                };
                if let Err(e) = booked {
                    return error(StatusCode::INTERNAL_SERVER_ERROR, e);
                }
                return error(StatusCode::BAD_GATEWAY, err);
            }
        };
        self.breakers.record_success(&domain);
        // A scan probe reads one byte at the start. Playback reads the file:
        // the size the headers reported, and a seek when it opened past the
        // start. The quota still counts requests only.
        let (bytes, seek) = if playback {
            (link.size, offset > 0)
        } else {
            (1, false)
        };
        let _ = force;
        if let Err(e) = self.note_read(&domain, bytes, i64::from(seek)) {
            return error(StatusCode::INTERNAL_SERVER_ERROR, e);
        }
        if let Err(e) = self.cache.clear_failure(target) {
            return error(StatusCode::INTERNAL_SERVER_ERROR, e);
        }
        let entry = Entry {
            target: target.to_owned(),
            direct: link.url.clone(),
            ua: self.ua.clone(),
            expires_at: link.expires_at,
            size: link.size,
            ..Default::default()
        };
        if let Err(e) = self.cache.put(&entry) {
            return error(StatusCode::INTERNAL_SERVER_ERROR, e);
        }
        json(&Resolved {
            direct: link.url,
            expires_at: link.expires_at,
            cached: force,
        })
    }

    /// Books one cloud read. `bytes` is the size the headers reported.
    /// `offset` above zero is a seek: the caller opened past the start.
    /// The quota is not touched here.
    fn note_read(&self, domain: &str, bytes: i64, offset: i64) -> rusqlite::Result<()> {
        self.cache
            .record_read(domain, bytes, offset > 0, SystemTime::now())
    }
}

async fn handle_resolve(State(d): State<Arc<Daemon>>, body: Bytes) -> Response {
    let req = match decode(&body) {
        Ok(req) => req,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    match d.cache.get(&req.target) {
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, e),
        Ok(Some(entry)) => {
            // A cache hit on the scan path reads nothing new: the probe
            // already counted its one byte. Playback passes where it opened,
            // and that open is the read, from the start or from later on.
            // The quota is not touched.
            if req.playback
                && let Err(e) = d.note_read(&host_of(&req.target), entry.size, req.offset)
            {
                return error(StatusCode::INTERNAL_SERVER_ERROR, e);
            }
            return json(&Resolved {
                direct: entry.direct,
                expires_at: entry.expires_at,
                cached: true,
            });
        }
        Ok(None) => {}
    }

    // gatekeeping for scan-driven resolutions only
    let now = Instant::now();
    let domain = host_of(&req.target);
    let admit = d.breakers.allow(&domain, now, false);
    if admit == Admit::Reject {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("breaker open for {domain}"),
        );
    }
    // a probe refused below never reached the upstream: hand its slot
    // back, or the breaker would stay half-open and reject every scan
    let refuse = |status, msg: String| {
        if admit == Admit::Probe {
            d.breakers.release_probe(&domain);
        }
        error(status, msg)
    };
    match d.cache.should_skip(&req.target, SystemTime::now()) {
        Err(e) => return refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        Ok(true) => return refuse(StatusCode::TOO_MANY_REQUESTS, "target backing off".into()),
        Ok(false) => {}
    }
    if !d.quota.allow(&domain, now) {
        return refuse(
            StatusCode::TOO_MANY_REQUESTS,
            format!("quota exceeded for {domain}"),
        );
    }
    d.resolve_and_store(&req.target, false, req.offset, req.playback)
        .await
}

/// Re-resolves unconditionally; called when playback hit a dead link.
/// Never called in bulk.
async fn handle_refresh(State(d): State<Arc<Daemon>>, body: Bytes) -> Response {
    match decode(&body) {
        Ok(req) => {
            d.resolve_and_store(&req.target, true, req.offset, true)
                .await
        }
        Err(e) => error(StatusCode::BAD_REQUEST, e),
    }
}

/// Records a dead cached link: a probe or playback failed on it even
/// after a refresh. Books the ledger; never resolves. `permanent`
/// skips the backoff and is not retried until a success clears it.
async fn handle_dead(State(d): State<Arc<Daemon>>, body: Bytes) -> Response {
    match decode(&body) {
        Ok(req) => {
            let booked = if req.permanent {
                d.cache.record_permanent(&req.target, SystemTime::now())
            } else {
                d.cache.record_failure(&req.target, SystemTime::now())
            };
            match booked {
                Ok(()) => StatusCode::NO_CONTENT.into_response(),
                Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e),
            }
        }
        Err(e) => error(StatusCode::BAD_REQUEST, e),
    }
}

async fn handle_state(State(d): State<Arc<Daemon>>) -> Response {
    let stats = match d.cache.stats() {
        Ok(stats) => stats,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    json(&StateReport {
        breakers: d.breakers.snapshot(),
        entries: stats.entries,
        failures: d.cache.failure_count().unwrap_or(0),
        hits: stats.hits,
        quota: d.quota.usage(Instant::now()),
    })
}

/// Extracts the upstream domain (host[:port]) of a target URL.
fn host_of(target: &str) -> String {
    let Ok(url) = Url::parse(target) else {
        return String::new();
    };
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    }
}

/// Parses the request body. Content-Type is deliberately not checked:
/// the Go client never set it.
fn decode(body: &[u8]) -> serde_json::Result<TargetRequest> {
    serde_json::from_slice(body)
}

/// A plain-text error with a trailing newline, like Go's http.Error.
fn error(status: StatusCode, msg: impl Display) -> Response {
    (status, format!("{msg}\n")).into_response()
}

/// A JSON body with a trailing newline, like Go's json.Encoder.
fn json(value: &impl Serialize) -> Response {
    let mut body = serde_json::to_vec(value).expect("response types always serialize");
    body.push(b'\n');
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::cache::tests::test_cache;
    use axum::extract::Path as UrlPath;
    use std::sync::atomic::{AtomicBool, Ordering};

    async fn spawn(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        base
    }

    fn partial_content() -> Response {
        (
            StatusCode::PARTIAL_CONTENT,
            [(header::CONTENT_RANGE, "bytes 0-0/10")],
            "0",
        )
            .into_response()
    }

    fn found(location: String) -> Response {
        (StatusCode::FOUND, [(header::LOCATION, location)]).into_response()
    }

    /// Posts without a Content-Type, exactly like the Go client.
    async fn post(base: &str, path: &str, target: &str) -> (StatusCode, String) {
        let body = serde_json::to_string(&TargetRequest {
            target: target.into(),
            ..Default::default()
        })
        .unwrap();
        let resp = reqwest::Client::new()
            .post(format!("{base}{path}"))
            .body(body)
            .send()
            .await
            .unwrap();
        (resp.status(), resp.text().await.unwrap())
    }

    async fn resolve_via(base: &str, target: &str) -> Resolved {
        let (status, body) = post(base, "/resolve", target).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.ends_with('\n'));
        serde_json::from_str(&body).unwrap()
    }

    /// A stored link stays trusted regardless of its stored expiry; no
    /// upstream request is ever made for a cache hit.
    #[tokio::test]
    async fn resolve_trusts_cache() {
        let (_dir, cache) = test_cache();
        let target = "http://x/file/ep1";
        let stale = "http://x/dl/ep1?t=stale";
        cache
            .put(&Entry {
                target: target.into(),
                direct: stale.into(),
                expires_at: 1,
                ..Default::default()
            })
            .unwrap();
        let base = spawn(Daemon::new(Arc::new(cache), "", 30).router()).await;

        let got = resolve_via(&base, target).await;
        assert!(got.cached && got.direct == stale, "{got:?}");
    }

    #[tokio::test]
    async fn resolve_then_cache_hit() {
        let exp = unix_now() + 3600;
        let upstream = spawn(
            Router::new()
                .route(
                    "/file/{id}",
                    get(move |UrlPath(id): UrlPath<String>| async move {
                        found(format!("/dl/{id}?t={exp}.sig"))
                    }),
                )
                .route("/dl/{id}", get(|| async { partial_content() })),
        )
        .await;
        let (_dir, cache) = test_cache();
        let cache = Arc::new(cache);
        let base = spawn(Daemon::new(Arc::clone(&cache), "", 30).router()).await;
        let target = format!("{upstream}/file/ep1");

        let got = resolve_via(&base, &target).await;
        assert!(!got.cached, "first resolve must not be cached");
        assert_eq!(got.expires_at, exp, "sniffed expiry");

        let again = resolve_via(&base, &target).await;
        assert!(again.cached && again.direct == got.direct, "{again:?}");

        let stats = cache.stats().unwrap();
        assert_eq!(stats.entries, 1);
        assert!(stats.hits >= 1);
    }

    /// Three failing scan resolves open the breaker; a playback refresh
    /// bypasses it and, once the upstream recovered, resets it.
    #[tokio::test]
    async fn guard_breaker() {
        let broken = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&broken);
        let upstream = spawn(
            Router::new()
                .route(
                    "/file/{id}",
                    get(move || async move {
                        if flag.load(Ordering::SeqCst) {
                            (StatusCode::INTERNAL_SERVER_ERROR, "boom").into_response()
                        } else {
                            found("/dl/ok".into())
                        }
                    }),
                )
                .route("/dl/{id}", get(|| async { partial_content() })),
        )
        .await;
        let (_dir, cache) = test_cache();
        let base = spawn(Daemon::new(Arc::new(cache), "", 30).router()).await;
        let target = |ep: &str| format!("{upstream}/file/{ep}");

        // an outage fails distinct targets; after three the breaker opens
        for ep in ["ep1", "ep2", "ep3"] {
            let (status, body) = post(&base, "/resolve", &target(ep)).await;
            assert_eq!(status, StatusCode::BAD_GATEWAY, "{ep}: {body}");
        }
        let (status, body) = post(&base, "/resolve", &target("ep4")).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body.starts_with("breaker open for 127.0.0.1:"), "{body}");

        // upstream recovers; playback refresh bypasses the open breaker
        broken.store(false, Ordering::SeqCst);
        let (status, body) = post(&base, "/refresh", &target("ep4")).await;
        assert_eq!(status, StatusCode::OK, "{body}");

        // the forced success closed the breaker for scans again
        assert!(!resolve_via(&base, &target("ep5")).await.cached);
        assert!(resolve_via(&base, &target("ep5")).await.cached);
    }

    /// A probe refused by the ledger must not wedge the breaker half-open:
    /// the next scan gets the probe and closes it again.
    #[tokio::test]
    async fn refused_probe_is_released() {
        use crate::runtime::breaker::BreakerConfig;

        let broken = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&broken);
        let upstream = spawn(
            Router::new()
                .route(
                    "/file/{id}",
                    get(move || async move {
                        if flag.load(Ordering::SeqCst) {
                            StatusCode::INTERNAL_SERVER_ERROR.into_response()
                        } else {
                            found("/dl/ok".into())
                        }
                    }),
                )
                .route("/dl/{id}", get(|| async { partial_content() })),
        )
        .await;
        let (_dir, cache) = test_cache();
        // zero cooldown: an open breaker turns half-open immediately
        let instant = BreakerConfig {
            threshold: 3,
            base_cool: Duration::ZERO,
            max_cool: Duration::ZERO,
        };
        let daemon = Arc::new(Daemon {
            cache: Arc::new(cache),
            ua: String::new(),
            breakers: Breakers::new(instant),
            quota: Quota::new(30, QUOTA_WINDOW),
        });
        let base = spawn(daemon.router()).await;
        let target = |ep: &str| format!("{upstream}/file/{ep}");

        for ep in ["ep1", "ep2", "ep3"] {
            let (status, _) = post(&base, "/resolve", &target(ep)).await;
            assert_eq!(status, StatusCode::BAD_GATEWAY, "{ep}");
        }
        broken.store(false, Ordering::SeqCst);

        // ep1 is backing off: it takes the probe slot, then the ledger refuses it
        let (status, _) = post(&base, "/resolve", &target("ep1")).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);

        // the slot was handed back, so a fresh target can probe and close it
        let (status, body) = post(&base, "/resolve", &target("ep9")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(daemon.breakers.snapshot().values().next(), Some(&"closed"));
    }

    #[tokio::test]
    async fn dead_target_is_not_retried() {
        let upstream =
            spawn(Router::new().route("/file/{id}", get(|| async { StatusCode::NOT_FOUND }))).await;
        let (_dir, cache) = test_cache();
        let cache = Arc::new(cache);
        let base = spawn(Daemon::new(Arc::clone(&cache), "", 30).router()).await;
        let target = format!("{upstream}/file/dead");

        let (status, body) = post(&base, "/resolve", &target).await;
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::BAD_GATEWAY, "dead link\n")
        );
        // a 404 does not clear on its own, so the skip has no expiry
        let week = SystemTime::now() + Duration::from_secs(8 * 24 * 60 * 60);
        assert!(cache.should_skip(&target, week).unwrap());
        let (status, body) = post(&base, "/resolve", &target).await;
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::TOO_MANY_REQUESTS, "target backing off\n")
        );
    }

    /// A refusal can be a signed link the upstream has not accepted yet,
    /// so it takes the backoff instead of being parked forever.
    #[tokio::test]
    async fn forbidden_target_backs_off() {
        let upstream =
            spawn(Router::new().route("/file/{id}", get(|| async { StatusCode::FORBIDDEN }))).await;
        let (_dir, cache) = test_cache();
        let cache = Arc::new(cache);
        let base = spawn(Daemon::new(Arc::clone(&cache), "", 30).router()).await;
        let target = format!("{upstream}/file/nope");

        let (status, _) = post(&base, "/resolve", &target).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert!(cache.should_skip(&target, SystemTime::now()).unwrap());
        let later = SystemTime::now() + Duration::from_secs(25 * 60 * 60);
        assert!(!cache.should_skip(&target, later).unwrap());
    }

    #[tokio::test]
    async fn quota_limits_scans() {
        let upstream = spawn(
            Router::new()
                .route(
                    "/file/{id}",
                    get(|UrlPath(id): UrlPath<String>| async move { found(format!("/dl/{id}")) }),
                )
                .route("/dl/{id}", get(|| async { partial_content() })),
        )
        .await;
        let (_dir, cache) = test_cache();
        let base = spawn(Daemon::new(Arc::new(cache), "", 1).router()).await;

        resolve_via(&base, &format!("{upstream}/file/a")).await;
        let (status, body) = post(&base, "/resolve", &format!("{upstream}/file/b")).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert!(body.starts_with("quota exceeded for 127.0.0.1:"), "{body}");
        // playback is never limited
        let (status, _) = post(&base, "/refresh", &format!("{upstream}/file/b")).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn bad_request() {
        let (_dir, cache) = test_cache();
        let base = spawn(Daemon::new(Arc::new(cache), "", 30).router()).await;
        let resp = reqwest::Client::new()
            .post(format!("{base}/resolve"))
            .body("{not json")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn state_report() {
        let (_dir, cache) = test_cache();
        cache
            .put(&Entry {
                target: "t".into(),
                direct: "d".into(),
                ..Default::default()
            })
            .unwrap();
        cache.get("t").unwrap();
        cache.record_failure("dead", SystemTime::now()).unwrap();
        let base = spawn(Daemon::new(Arc::new(cache), "", 30).router()).await;

        let body = reqwest::get(format!("{base}/state"))
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(
            body,
            "{\"breakers\":{},\"entries\":1,\"failures\":1,\"hits\":1,\"quota\":{}}\n"
        );
    }

    #[tokio::test]
    async fn serves_on_unix_socket() {
        let (dir, cache) = test_cache();
        let sock = dir.path().join("assistant.sock");
        fs::write(&sock, "stale").unwrap(); // a leftover file is replaced
        let daemon = Daemon::new(Arc::new(cache), "", 30);
        let path = sock.clone();
        tokio::spawn(async move { daemon.serve(path).await.unwrap() });

        let client = reqwest::Client::builder()
            .unix_socket(sock.clone())
            .build()
            .unwrap();
        let mut status = None;
        for _ in 0..50 {
            if let Ok(resp) = client.get("http://assistant/state").send().await {
                status = Some(resp.status());
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(status, Some(StatusCode::OK));
        // the socket is locked down to owner+group
        let mode = fs::metadata(&sock).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o660);
    }

    #[test]
    fn host_of_keeps_port() {
        assert_eq!(host_of("http://a.example/file/1"), "a.example");
        assert_eq!(host_of("http://a.example:8399/file/1"), "a.example:8399");
        assert_eq!(host_of("not a url"), "");
    }

    fn unix_now() -> i64 {
        crate::runtime::cache::unix_seconds(SystemTime::now())
    }
}

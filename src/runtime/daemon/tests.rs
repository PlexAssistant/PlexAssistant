use super::*;
use crate::runtime::api::TargetRequest;
use crate::runtime::cache::tests::test_cache;
use axum::extract::Path as UrlPath;
use axum::http::header;
use axum::response::IntoResponse;
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

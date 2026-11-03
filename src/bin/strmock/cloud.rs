//! A fake cloud drive: /file/{id} resolves to a signed direct link via
//! 302, /dl/{id} serves the bytes with injectable failure modes
//! (403/410/throttle) controlled at runtime via POST /ctl.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use hmac::{Hmac, KeyInit, Mac};
use serde::Deserialize;
use sha2::Sha256;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::duration;

const BEHAVIORS: [&str; 4] = ["normal", "forbidden", "gone", "slow"];

/// Runtime knobs set through /ctl.
#[derive(Debug, Default)]
struct Controls {
    /// id -> behavior
    behavior: BTreeMap<String, String>,
    /// id -> bytes per second
    bps: BTreeMap<String, i64>,
    /// id -> link TTL in nanoseconds
    ttl: BTreeMap<String, i64>,
    /// nanoseconds
    default_ttl: i64,
}

pub struct Server {
    secret: Vec<u8>,
    root: PathBuf,
    controls: Mutex<Controls>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CtlRequest {
    /// file id, or "*" for global reset/defaults
    id: String,
    /// normal | forbidden | gone | slow
    behavior: String,
    /// bytes per second for slow
    bps: i64,
    /// link TTL, e.g. "10s"
    ttl: String,
}

impl Server {
    pub fn new(secret: &str, root: impl Into<PathBuf>, default_ttl_ns: i64) -> Arc<Self> {
        Arc::new(Self {
            secret: secret.as_bytes().to_vec(),
            root: root.into(),
            controls: Mutex::new(Controls {
                default_ttl: default_ttl_ns,
                ..Default::default()
            }),
        })
    }

    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/file/{id}", get(handle_file))
            .route("/dl/{id}", get(handle_dl))
            .route("/ctl", post(handle_ctl))
            .route("/state", get(handle_state))
            .with_state(Arc::clone(self))
    }

    fn controls(&self) -> MutexGuard<'_, Controls> {
        self.controls.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Returns `<unix-exp>.<hmac8>` over id and exp.
    fn sign(&self, id: &str, exp: i64) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.secret).expect("hmac takes any key");
        mac.update(format!("{id}|{exp}").as_bytes());
        let tag: String = mac.finalize().into_bytes()[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        format!("{exp}.{tag}")
    }

    /// Checks the token signature and returns the expiry.
    fn verify(&self, id: &str, token: &str) -> Option<i64> {
        let exp = token.split_once('.')?.0.parse().ok()?;
        constant_time_eq(self.sign(id, exp).as_bytes(), token.as_bytes()).then_some(exp)
    }
}

async fn handle_file(State(s): State<Arc<Server>>, Path(id): Path<String>) -> Response {
    if !valid_id(&id) || !s.root.join(&id).exists() {
        return not_found();
    }
    let ttl = {
        let c = s.controls();
        c.ttl.get(&id).copied().unwrap_or(c.default_ttl)
    };
    let exp = (now_ns() + i128::from(ttl)).div_euclid(1_000_000_000) as i64;
    let location = format!("/dl/{}?t={}", encode_segment(&id), s.sign(&id, exp));
    (StatusCode::FOUND, [(header::LOCATION, location)]).into_response()
}

/// Serves the bytes after token validation and behavior injection.
async fn handle_dl(
    State(s): State<Arc<Server>>,
    Path(id): Path<String>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    if !valid_id(&id) {
        return not_found();
    }
    let Ok(file) = File::open(s.root.join(&id)).await else {
        return not_found();
    };
    let token = url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes())
        .find(|(k, _)| k == "t")
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default();
    let Some(exp) = s.verify(&id, &token) else {
        return text(StatusCode::FORBIDDEN, "invalid token");
    };
    if now_ns() > i128::from(exp) * 1_000_000_000 {
        return text(StatusCode::GONE, "link expired");
    }
    let (behavior, bps) = {
        let c = s.controls();
        (
            c.behavior.get(&id).cloned().unwrap_or_default(),
            c.bps.get(&id).copied().unwrap_or(0),
        )
    };
    match behavior.as_str() {
        "forbidden" => return text(StatusCode::FORBIDDEN, "forbidden"),
        "gone" => return text(StatusCode::GONE, "gone"),
        _ => {}
    }
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    serve_file(file, &id, range, bps)
        .await
        .unwrap_or_else(|e| text(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))
}

async fn handle_ctl(State(s): State<Arc<Server>>, body: Bytes) -> Response {
    let req: CtlRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => return text(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    let ttl = if req.ttl.is_empty() {
        None
    } else {
        match duration::parse(&req.ttl) {
            Ok(d) if d >= 0 => Some(d),
            _ => return text(StatusCode::BAD_REQUEST, "bad ttl"),
        }
    };
    if !req.behavior.is_empty() && !BEHAVIORS.contains(&req.behavior.as_str()) {
        return text(StatusCode::BAD_REQUEST, "bad behavior");
    }

    let mut c = s.controls();
    if req.id == "*" {
        if req.behavior == "normal" {
            c.behavior.clear();
            c.bps.clear();
        }
        if let Some(ttl) = ttl {
            c.default_ttl = ttl;
        }
    } else {
        if !req.behavior.is_empty() {
            c.behavior.insert(req.id.clone(), req.behavior);
        }
        if req.bps > 0 {
            c.bps.insert(req.id.clone(), req.bps);
        } else {
            c.bps.remove(&req.id);
        }
        if let Some(ttl) = ttl {
            c.ttl.insert(req.id, ttl);
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn handle_state(State(s): State<Arc<Server>>) -> Response {
    let c = s.controls();
    let mut body = serde_json::to_vec(&serde_json::json!({
        "behavior": c.behavior,
        "bps": c.bps,
        "default_ttl": duration::format(c.default_ttl),
        "ttl": c.ttl,
    }))
    .expect("state always serializes");
    body.push(b'\n');
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

/// Serves the file, honoring a single byte range; a multi-range request
/// gets the whole file, which RFC 9110 allows.
async fn serve_file(
    mut file: File,
    id: &str,
    range: Option<&str>,
    bps: i64,
) -> io::Result<Response> {
    let size = file.metadata().await?.len();
    let mut builder = Response::builder()
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_TYPE, content_type(id));
    let (start, len) = match range.map(|r| parse_range(r, size)) {
        None | Some(Range::Ignored) => {
            builder = builder.status(StatusCode::OK);
            (0, size)
        }
        Some(Range::Unsatisfiable) => {
            return Ok(Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{size}"))
                .body(Body::empty())
                .expect("valid response"));
        }
        Some(Range::Bytes(start, end)) => {
            builder = builder
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
            (start, end - start + 1)
        }
    };
    file.seek(io::SeekFrom::Start(start)).await?;
    Ok(builder
        .header(header::CONTENT_LENGTH, len)
        .body(stream_body(file, len, bps))
        .expect("valid response"))
}

/// Streams len bytes; with bps set, sends a bps/10 chunk every 100ms.
fn stream_body(file: File, len: u64, bps: i64) -> Body {
    let chunk = if bps > 0 {
        (bps / 10).max(1) as u64
    } else {
        64 * 1024
    };
    let stream = futures_util::stream::unfold((file, len), move |(mut file, left)| async move {
        if left == 0 {
            return None;
        }
        let mut buf = vec![0; chunk.min(left) as usize];
        match file.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                if bps > 0 {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Some((Ok(Bytes::from(buf)), (file, left - n as u64)))
            }
            Err(e) => Some((Err::<Bytes, io::Error>(e), (file, 0))),
        }
    });
    Body::from_stream(stream)
}

#[derive(Debug, PartialEq, Eq)]
enum Range {
    /// inclusive byte positions
    Bytes(u64, u64),
    Unsatisfiable,
    Ignored,
}

fn parse_range(header: &str, size: u64) -> Range {
    let Some(spec) = header.strip_prefix("bytes=") else {
        return Range::Unsatisfiable;
    };
    if spec.contains(',') {
        return Range::Ignored;
    }
    let Some((first, last)) = spec.trim().split_once('-') else {
        return Range::Unsatisfiable;
    };
    let (first, last) = (first.trim(), last.trim());
    let parsed = if first.is_empty() {
        // suffix range: the last N bytes
        last.parse::<u64>()
            .ok()
            .filter(|&n| n > 0 && size > 0)
            .map(|n| (size - n.min(size), size - 1))
    } else {
        first
            .parse::<u64>()
            .ok()
            .filter(|&f| f < size)
            .and_then(|f| {
                match last {
                    "" => Some(size - 1),
                    l => l
                        .parse::<u64>()
                        .ok()
                        .filter(|&l| l >= f)
                        .map(|l| l.min(size - 1)),
                }
                .map(|l| (f, l))
            })
    };
    parsed.map_or(Range::Unsatisfiable, |(f, l)| Range::Bytes(f, l))
}

fn content_type(id: &str) -> &'static str {
    match id
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("mkv") => "video/x-matroska",
        Some("mp4" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("ts") => "video/mp2t",
        Some("avi") => "video/x-msvideo",
        Some("mp3") => "audio/mpeg",
        Some("srt") => "application/x-subrip",
        _ => "application/octet-stream",
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id != "." && id != ".." && !id.contains(['/', '\\'])
}

/// Percent-encodes a path segment so ids with spaces stay valid URLs.
fn encode_segment(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn now_ns() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i128)
}

fn not_found() -> Response {
    text(StatusCode::NOT_FOUND, "404 page not found")
}

/// A plain-text body with a trailing newline, like Go's http.Error.
fn text(status: StatusCode, msg: &str) -> Response {
    (status, format!("{msg}\n")).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    const TEST_BODY: &str = "0123456789abcdef";
    const HOUR: i64 = 3_600_000_000_000;

    struct Cloud {
        _root: tempfile::TempDir,
        base: String,
        http: reqwest::Client,
    }

    impl Cloud {
        async fn start(default_ttl: i64) -> Self {
            let root = tempfile::tempdir().unwrap();
            std::fs::write(root.path().join("ep1.mkv"), TEST_BODY).unwrap();
            std::fs::write(root.path().join("my movie.mkv"), TEST_BODY).unwrap();
            let app = Server::new("k", root.path(), default_ttl).router();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            Self {
                _root: root,
                base,
                http: reqwest::Client::new(),
            }
        }

        async fn get(&self, path: &str) -> reqwest::Response {
            self.http
                .get(format!("{}{path}", self.base))
                .send()
                .await
                .unwrap()
        }

        async fn ctl(&self, body: &str) -> StatusCode {
            self.http
                .post(format!("{}/ctl", self.base))
                .body(body.to_owned())
                .send()
                .await
                .unwrap()
                .status()
        }
    }

    #[tokio::test]
    async fn resolve_and_download() {
        let cloud = Cloud::start(HOUR).await;
        let resp = cloud.get("/file/ep1.mkv").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()[header::CONTENT_TYPE], "video/x-matroska");
        let url = resp.url().to_string();
        assert!(url.contains("/dl/ep1.mkv?t="), "{url}");
        assert_eq!(resp.text().await.unwrap(), TEST_BODY);
    }

    #[tokio::test]
    async fn ids_with_spaces_redirect_to_valid_urls() {
        let cloud = Cloud::start(HOUR).await;
        let resp = cloud.get("/file/my%20movie.mkv").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.url().path().ends_with("/dl/my%20movie.mkv"));
    }

    #[tokio::test]
    async fn range_requests() {
        let cloud = Cloud::start(HOUR).await;
        let direct = cloud.get("/file/ep1.mkv").await.url().to_string();
        let range = |r: &'static str| {
            let req = cloud.http.get(&direct).header(header::RANGE, r);
            async move { req.send().await.unwrap() }
        };

        let resp = range("bytes=0-0").await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(resp.headers()[header::CONTENT_RANGE], "bytes 0-0/16");
        assert_eq!(resp.text().await.unwrap(), "0");

        let resp = range("bytes=-4").await;
        assert_eq!(resp.headers()[header::CONTENT_RANGE], "bytes 12-15/16");
        assert_eq!(resp.text().await.unwrap(), "cdef");

        let resp = range("bytes=10-").await;
        assert_eq!(resp.text().await.unwrap(), "abcdef");

        let resp = range("bytes=99-").await;
        assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(resp.headers()[header::CONTENT_RANGE], "bytes */16");

        let resp = range("bytes=0-1,4-5").await;
        assert_eq!(resp.status(), StatusCode::OK, "multi-range serves it all");
    }

    #[tokio::test]
    async fn invalid_token() {
        let cloud = Cloud::start(HOUR).await;
        let resp = cloud.get("/dl/ep1.mkv?t=99999999.deadbeefdeadbeef").await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            cloud.get("/dl/ep1.mkv").await.status(),
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn expired_link() {
        let cloud = Cloud::start(-60_000_000_000).await;
        assert_eq!(cloud.get("/file/ep1.mkv").await.status(), StatusCode::GONE);
    }

    #[tokio::test]
    async fn forbidden_and_gone_behaviors() {
        let cloud = Cloud::start(HOUR).await;
        assert_eq!(
            cloud
                .ctl(r#"{"id":"ep1.mkv","behavior":"forbidden"}"#)
                .await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            cloud.get("/file/ep1.mkv").await.status(),
            StatusCode::FORBIDDEN
        );
        cloud.ctl(r#"{"id":"ep1.mkv","behavior":"gone"}"#).await;
        assert_eq!(cloud.get("/file/ep1.mkv").await.status(), StatusCode::GONE);
    }

    #[tokio::test]
    async fn ctl_reset() {
        let cloud = Cloud::start(HOUR).await;
        cloud
            .ctl(r#"{"id":"ep1.mkv","behavior":"forbidden"}"#)
            .await;
        cloud.ctl(r#"{"id":"*","behavior":"normal"}"#).await;
        assert_eq!(cloud.get("/file/ep1.mkv").await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ctl_validation() {
        let cloud = Cloud::start(HOUR).await;
        assert_eq!(cloud.ctl("{not json").await, StatusCode::BAD_REQUEST);
        assert_eq!(
            cloud.ctl(r#"{"id":"a","ttl":"-1s"}"#).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            cloud.ctl(r#"{"id":"a","ttl":"soon"}"#).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            cloud.ctl(r#"{"id":"a","behavior":"odd"}"#).await,
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn per_id_ttl_expires_links() {
        let cloud = Cloud::start(HOUR).await;
        // a zero TTL signs the current second, which has begun already
        cloud.ctl(r#"{"id":"ep1.mkv","ttl":"0s"}"#).await;
        assert_eq!(cloud.get("/file/ep1.mkv").await.status(), StatusCode::GONE);
    }

    #[tokio::test]
    async fn throttles_slow_ids() {
        let cloud = Cloud::start(HOUR).await;
        // 80 B/s: 8-byte chunks every 100ms, so 16 bytes take >= 200ms
        cloud
            .ctl(r#"{"id":"ep1.mkv","behavior":"slow","bps":80}"#)
            .await;
        let started = Instant::now();
        let body = cloud.get("/file/ep1.mkv").await.text().await.unwrap();
        assert_eq!(body, TEST_BODY);
        assert!(
            started.elapsed() >= Duration::from_millis(200),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn state_report() {
        let cloud = Cloud::start(HOUR).await;
        cloud
            .ctl(r#"{"id":"ep1.mkv","behavior":"slow","bps":80,"ttl":"10s"}"#)
            .await;
        let body = cloud.get("/state").await.text().await.unwrap();
        assert_eq!(
            body,
            "{\"behavior\":{\"ep1.mkv\":\"slow\"},\"bps\":{\"ep1.mkv\":80},\
             \"default_ttl\":\"1h0m0s\",\"ttl\":{\"ep1.mkv\":10000000000}}\n"
        );
    }

    #[tokio::test]
    async fn missing_and_invalid_ids() {
        let cloud = Cloud::start(HOUR).await;
        let resp = cloud.get("/file/missing.mkv").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(resp.text().await.unwrap(), "404 page not found\n");
        assert_eq!(cloud.get("/file/..").await.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            cloud.get("/file/a%2Fb").await.status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn tokens() {
        let s = Server::new("k", "/", HOUR);
        let token = s.sign("ep1.mkv", 1_800_000_000);
        assert_eq!(token.len(), "1800000000.".len() + 16);
        assert_eq!(s.verify("ep1.mkv", &token), Some(1_800_000_000));
        assert_eq!(s.verify("ep2.mkv", &token), None);
        assert_eq!(s.verify("ep1.mkv", "1800000001.0000000000000000"), None);
        assert_eq!(s.verify("ep1.mkv", "garbage"), None);
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-0", 16), Range::Bytes(0, 0));
        assert_eq!(parse_range("bytes=4-100", 16), Range::Bytes(4, 15));
        assert_eq!(parse_range("bytes=-100", 16), Range::Bytes(0, 15));
        assert_eq!(parse_range("bytes=5-4", 16), Range::Unsatisfiable);
        assert_eq!(parse_range("bytes=-0", 16), Range::Unsatisfiable);
        assert_eq!(parse_range("items=0-1", 16), Range::Unsatisfiable);
        assert_eq!(parse_range("bytes=0-0", 0), Range::Unsatisfiable);
    }
}

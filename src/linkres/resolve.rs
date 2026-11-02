use std::sync::OnceLock;
use std::time::SystemTime;

use reqwest::header::{CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, LOCATION, RANGE, USER_AGENT};
use reqwest::redirect::Policy;
use reqwest::{Client, StatusCode};
use url::Url;

use super::expiry::sniff_expiry;

/// Identifies us to cloud drives; links can be bound to it, so scan and
/// playback must use the same value.
pub const DEFAULT_UA: &str = "PlexAssistant/0.1";

/// Requests per resolution, the first one included (Go's limit).
const MAX_REQUESTS: usize = 10;

/// Resolution failure kinds; the scanner maps them to its failure states.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("dead link")]
    Dead,
    #[error("forbidden")]
    Forbidden,
    #[error("gone")]
    Gone,
    #[error("unexpected status {0}")]
    UnexpectedStatus(u16),
    #[error("too many redirects")]
    TooManyRedirects,
    #[error("invalid url: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}

/// A resolved direct link plus what the probe learned.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Link {
    /// final direct link
    pub url: String,
    /// redirect hops taken
    pub chain: Vec<String>,
    pub content_type: String,
    /// total bytes if known
    pub size: i64,
    /// server answered 206 to our range probe
    pub range_supported: bool,
    /// unix seconds sniffed from the URL, 0 = unknown
    pub expires_at: i64,
}

static CLIENT: OnceLock<Client> = OnceLock::new();

/// One client for every resolution, so connections and TLS sessions to
/// a cloud drive are reused across a section scan. Redirects are followed
/// by hand to record the chain per call.
fn client() -> Result<&'static Client, ResolveError> {
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let client = Client::builder().redirect(Policy::none()).build()?;
    Ok(CLIENT.get_or_init(|| client))
}

/// Follows the strm URL's redirect chain and returns the direct link
/// without downloading the media: it probes with a 1-byte range request
/// and reads only headers.
pub async fn resolve(raw_url: &str, ua: &str) -> Result<Link, ResolveError> {
    let ua = if ua.is_empty() { DEFAULT_UA } else { ua };
    let client = client()?;

    let mut url = Url::parse(raw_url)?;
    let mut chain = Vec::new();
    let mut requests = 0;
    let resp = loop {
        requests += 1;
        let resp = client
            .get(url.clone())
            .header(USER_AGENT, ua)
            .header(RANGE, "bytes=0-0")
            .send()
            .await?;
        let Some(next) = redirect_target(&url, &resp) else {
            break resp;
        };
        chain.push(next.to_string());
        if requests >= MAX_REQUESTS {
            return Err(ResolveError::TooManyRedirects);
        }
        url = next;
    };

    match resp.status() {
        StatusCode::OK | StatusCode::PARTIAL_CONTENT => {}
        StatusCode::NOT_FOUND => return Err(ResolveError::Dead),
        StatusCode::FORBIDDEN => return Err(ResolveError::Forbidden),
        StatusCode::GONE => return Err(ResolveError::Gone),
        other => return Err(ResolveError::UnexpectedStatus(other.as_u16())),
    }

    let header = |name| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
    };
    let url = resp.url().to_string();
    let size = match content_range_total(header(CONTENT_RANGE)) {
        0 => header(CONTENT_LENGTH).parse().unwrap_or(0),
        total => total,
    };
    Ok(Link {
        expires_at: sniff_expiry(&url, SystemTime::now()),
        chain,
        content_type: header(CONTENT_TYPE).to_owned(),
        range_supported: resp.status() == StatusCode::PARTIAL_CONTENT,
        size,
        url,
    })
}

/// Where a redirect response points, resolved against the request URL;
/// None for anything else, including a redirect without a usable
/// Location (then reported as an unexpected status).
fn redirect_target(base: &Url, resp: &reqwest::Response) -> Option<Url> {
    let followed = matches!(resp.status().as_u16(), 301 | 302 | 303 | 307 | 308);
    if !followed {
        return None;
    }
    let location = resp.headers().get(LOCATION)?.to_str().ok()?;
    base.join(location).ok()
}

/// Parses "bytes 0-0/12345" into 12345; 0 when absent or malformed.
fn content_range_total(v: &str) -> i64 {
    v.split_once('/')
        .and_then(|(_, total)| total.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::extract::{Path, State};
    use axum::http::{HeaderMap, StatusCode, header};
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;

    type CapturedUa = Arc<Mutex<String>>;

    /// Simulates a cloud drive: /file redirects to /dl, which serves bytes
    /// with range support; behavior ids answer the failure codes.
    async fn cloud() -> (String, CapturedUa) {
        let ua = CapturedUa::default();
        let app = Router::new()
            .route("/file/{id}", get(redirect_to_dl))
            .route("/loop", get(|| async { found("/loop") }))
            .route("/hop/{n}", get(hop))
            .route(
                "/see-other",
                get(|| async {
                    (StatusCode::SEE_OTHER, [(header::LOCATION, "/dl/b.mkv")]).into_response()
                }),
            )
            .route(
                "/no-location",
                get(|| async { StatusCode::FOUND.into_response() }),
            )
            .route("/dl/{id}", get(download))
            .with_state(Arc::clone(&ua));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, ua)
    }

    fn found(location: &str) -> Response {
        (StatusCode::FOUND, [(header::LOCATION, location.to_owned())]).into_response()
    }

    /// /hop/2 -> /hop/1 -> absolute URL of /dl/x.mkv
    async fn hop(Path(n): Path<u32>, headers: HeaderMap) -> Response {
        if n == 0 {
            let host = headers[header::HOST].to_str().unwrap().to_owned();
            return found(&format!("http://{host}/dl/x.mkv"));
        }
        found(&format!("{}", n - 1))
    }

    async fn redirect_to_dl(Path(id): Path<String>) -> Response {
        found(&format!("/dl/{id}"))
    }

    async fn download(
        State(ua): State<CapturedUa>,
        Path(id): Path<String>,
        headers: HeaderMap,
    ) -> Response {
        let header = |name| {
            headers
                .get(name)
                .and_then(|v: &header::HeaderValue| v.to_str().ok())
                .map(str::to_owned)
        };
        *ua.lock().unwrap() = header(header::USER_AGENT).unwrap_or_default();
        let status = match id.as_str() {
            "forbidden" => Some(StatusCode::FORBIDDEN),
            "gone" => Some(StatusCode::GONE),
            "missing" => Some(StatusCode::NOT_FOUND),
            "broken" => Some(StatusCode::INTERNAL_SERVER_ERROR),
            _ => None,
        };
        if let Some(status) = status {
            return status.into_response();
        }
        let body = "0123456789";
        if id != "norange" && header(header::RANGE).is_some() {
            let range = format!("bytes 0-0/{}", body.len());
            return (
                StatusCode::PARTIAL_CONTENT,
                [(header::CONTENT_RANGE, range)],
                &body[..1],
            )
                .into_response();
        }
        body.into_response()
    }

    #[tokio::test]
    async fn follows_redirects() {
        let (base, ua) = cloud().await;
        let link = resolve(&format!("{base}/file/ep1.mkv"), DEFAULT_UA)
            .await
            .unwrap();
        let direct = format!("{base}/dl/ep1.mkv");
        assert_eq!(link.url, direct);
        assert_eq!(link.chain, [direct]);
        assert!(link.range_supported, "{link:?}");
        assert_eq!(link.size, 10, "{link:?}");
        assert_eq!(*ua.lock().unwrap(), DEFAULT_UA);
    }

    #[tokio::test]
    async fn empty_ua_uses_default() {
        let (base, ua) = cloud().await;
        resolve(&format!("{base}/dl/a.mkv"), "").await.unwrap();
        assert_eq!(*ua.lock().unwrap(), DEFAULT_UA);
    }

    #[tokio::test]
    async fn without_range_support() {
        let (base, _) = cloud().await;
        let link = resolve(&format!("{base}/dl/norange"), "").await.unwrap();
        assert!(!link.range_supported);
        assert_eq!(link.size, 10, "size from content-length");
        assert!(link.chain.is_empty());
    }

    #[tokio::test]
    async fn failures() {
        let (base, _) = cloud().await;
        // Display texts match Go's sentinel errors
        for (id, want) in [
            ("missing", "dead link"),
            ("forbidden", "forbidden"),
            ("gone", "gone"),
            ("broken", "unexpected status 500"),
        ] {
            let err = resolve(&format!("{base}/file/{id}"), "").await.unwrap_err();
            assert_eq!(err.to_string(), want, "{id}: {err:?}");
        }
    }

    #[tokio::test]
    async fn follows_relative_and_absolute_hops() {
        let (base, _) = cloud().await;
        let link = resolve(&format!("{base}/hop/2"), "").await.unwrap();
        assert_eq!(
            link.chain,
            [
                format!("{base}/hop/1"),
                format!("{base}/hop/0"),
                format!("{base}/dl/x.mkv")
            ]
        );
        assert_eq!(link.url, format!("{base}/dl/x.mkv"));

        let link = resolve(&format!("{base}/see-other"), "").await.unwrap();
        assert_eq!(link.url, format!("{base}/dl/b.mkv"));
    }

    /// Consecutive resolutions against one host share a connection.
    #[tokio::test]
    async fn reuses_connections() {
        use axum::extract::ConnectInfo;
        use std::net::SocketAddr;

        let peers = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&peers);
        let app = Router::new().route(
            "/dl/{id}",
            get(move |ConnectInfo(peer): ConnectInfo<SocketAddr>| {
                seen.lock().unwrap().push(peer.port());
                async { "0" }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let app = app.into_make_service_with_connect_info::<SocketAddr>();
            axum::serve(listener, app).await.unwrap()
        });

        for id in ["a", "b", "c"] {
            resolve(&format!("{base}/dl/{id}"), "").await.unwrap();
        }
        let peers = peers.lock().unwrap();
        assert_eq!(peers.len(), 3);
        assert!(peers.iter().all(|&p| p == peers[0]), "ports {peers:?}");
    }

    #[tokio::test]
    async fn redirect_without_location() {
        let (base, _) = cloud().await;
        let err = resolve(&format!("{base}/no-location"), "")
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "unexpected status 302");
    }

    #[tokio::test]
    async fn invalid_url() {
        let err = resolve("not a url", "").await.unwrap_err();
        assert!(matches!(err, ResolveError::InvalidUrl(_)), "{err:?}");
    }

    #[tokio::test]
    async fn too_many_redirects() {
        let (base, _) = cloud().await;
        let err = resolve(&format!("{base}/loop"), "").await.unwrap_err();
        assert!(matches!(err, ResolveError::TooManyRedirects), "{err:?}");
    }

    #[test]
    fn content_range() {
        assert_eq!(content_range_total("bytes 0-0/12345"), 12345);
        assert_eq!(content_range_total("bytes 0-0/*"), 0);
        assert_eq!(content_range_total(""), 0);
    }
}

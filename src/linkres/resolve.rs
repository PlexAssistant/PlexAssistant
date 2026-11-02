use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use reqwest::StatusCode;
use reqwest::header::{CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE, USER_AGENT};
use reqwest::redirect::Policy;

use super::expiry::sniff_expiry;

/// Identifies us to cloud drives; links can be bound to it, so scan and
/// playback must use the same value.
pub const DEFAULT_UA: &str = "PlexAssistant/0.1";

const MAX_REDIRECTS: usize = 10;

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

/// Follows the strm URL's redirect chain and returns the direct link
/// without downloading the media: it probes with a 1-byte range request
/// and reads only headers.
pub async fn resolve(raw_url: &str, ua: &str) -> Result<Link, ResolveError> {
    let ua = if ua.is_empty() { DEFAULT_UA } else { ua };

    let chain = Arc::new(Mutex::new(Vec::new()));
    let hops = Arc::clone(&chain);
    let client = reqwest::Client::builder()
        .redirect(Policy::custom(move |attempt| {
            hops.lock().unwrap().push(attempt.url().to_string());
            if attempt.previous().len() >= MAX_REDIRECTS {
                attempt.error("too many redirects")
            } else {
                attempt.follow()
            }
        }))
        .build()?;

    let resp = client
        .get(raw_url)
        .header(USER_AGENT, ua)
        .header(RANGE, "bytes=0-0")
        .send()
        .await?;

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
        chain: std::mem::take(&mut chain.lock().unwrap()),
        content_type: header(CONTENT_TYPE).to_owned(),
        range_supported: resp.status() == StatusCode::PARTIAL_CONTENT,
        size,
        url,
    })
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
    async fn too_many_redirects() {
        let (base, _) = cloud().await;
        let err = resolve(&format!("{base}/loop"), "").await.unwrap_err();
        assert!(
            matches!(&err, ResolveError::Http(e) if e.is_redirect()),
            "{err:?}"
        );
    }

    #[test]
    fn content_range() {
        assert_eq!(content_range_total("bytes 0-0/12345"), 12345);
        assert_eq!(content_range_total("bytes 0-0/*"), 0);
        assert_eq!(content_range_total(""), 0);
    }
}

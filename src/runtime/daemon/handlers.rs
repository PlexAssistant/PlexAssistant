//! The HTTP handlers, and the plain-text and JSON answers they share.

use std::collections::BTreeMap;
use std::fmt::Display;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use url::Url;

use super::Daemon;
use crate::runtime::api::{Resolved, TargetRequest};
use crate::runtime::breaker::Admit;

/// The /state report; fields in key order, matching Go's sorted maps.
#[derive(Serialize)]
struct StateReport {
    breakers: BTreeMap<String, &'static str>,
    entries: i64,
    failures: i64,
    hits: i64,
    quota: BTreeMap<String, usize>,
}

pub(super) async fn handle_resolve(State(d): State<Arc<Daemon>>, body: Bytes) -> Response {
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
                && let Err(e) = d.note_read(&host_of(&req.target), entry.size, req.offset > 0)
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

/// Re-resolves unconditionally; called when playback hit a dead link, or
/// once per target when a scan found its cached link dead. Never called
/// in bulk.
pub(super) async fn handle_refresh(State(d): State<Arc<Daemon>>, body: Bytes) -> Response {
    match decode(&body) {
        Ok(req) => {
            d.resolve_and_store(&req.target, true, req.offset, req.playback)
                .await
        }
        Err(e) => error(StatusCode::BAD_REQUEST, e),
    }
}

/// Records a dead cached link: a probe or playback failed on it even
/// after a refresh. Books the ledger; never resolves. `permanent`
/// skips the backoff and is not retried until a success clears it.
pub(super) async fn handle_dead(State(d): State<Arc<Daemon>>, body: Bytes) -> Response {
    match decode(&body) {
        Ok(req) => {
            match d
                .cache
                .record_dead(&req.target, req.permanent, SystemTime::now())
            {
                Ok(()) => StatusCode::NO_CONTENT.into_response(),
                Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e),
            }
        }
        Err(e) => error(StatusCode::BAD_REQUEST, e),
    }
}

pub(super) async fn handle_state(State(d): State<Arc<Daemon>>) -> Response {
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
pub(super) fn host_of(target: &str) -> String {
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
pub(super) fn error(status: StatusCode, msg: impl Display) -> Response {
    (status, format!("{msg}\n")).into_response()
}

/// A JSON body with a trailing newline, like Go's json.Encoder.
pub(super) fn json(value: &impl Serialize) -> Response {
    let mut body = serde_json::to_vec(value).expect("response types always serialize");
    body.push(b'\n');
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

//! Resolving a strm target to a direct link, and healing a dead one.
//!
//! The daemon is asked first, so its persistent cache saves upstream
//! resolutions; without it the target is resolved directly. Failures are
//! booked with the daemon so later scans back off.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use crate::linkres::{self, Link};
use crate::runtime::{Client, Resolved};
use crate::scanner::probe::{ProbeError, ProbeResult, probe};
use crate::scanner::redact_urls;

use super::Options;

/// One daemon client per process: section scans resolve every item
/// through the daemon, and rebuilding the HTTP client (and its
/// connection pool) per item is pure waste. Keyed by socket so tests
/// pointing at different daemons keep working.
static SHARED_CLIENT: OnceLock<(PathBuf, Client)> = OnceLock::new();

fn shared_client(sock: &Path) -> Option<Client> {
    if let Some((cached_sock, client)) = SHARED_CLIENT.get() {
        if cached_sock == sock {
            return Some(client.clone());
        }
        // a different socket (tests with several daemons): uncached
        return Client::new(sock).ok();
    }
    let client = Client::new(sock).ok()?;
    let _ = SHARED_CLIENT.set((sock.to_owned(), client.clone()));
    Some(client)
}

/// Probes the resolved link. A dead one (upstream 4xx) gets a single
/// deduped refresh per run — bounded by the number of distinct targets,
/// never by the number of items — and a target that stays dead is booked
/// into the ledger so later rounds back off instead of re-probing.
pub(super) async fn probe_and_heal(
    link: &Link,
    target: &str,
    opts: &Options,
    refreshed: &Mutex<HashSet<String>>,
    item: i64,
    part: i64,
) -> Option<(ProbeResult, Link)> {
    match probe(&opts.ffprobe, &link.url, &opts.user_agent).await {
        Ok(probed) => Some((probed, link.clone())),
        Err(e) if is_dead_link(&e) => {
            let claimed = refreshed
                .lock()
                .is_ok_and(|mut set| set.insert(target.to_owned()));
            if !claimed {
                report(target, opts, false).await;
                println!("item {item} part {part}: dead target, backing off");
                return None;
            }
            let fresh = match refresh_link(target, opts).await {
                Some(fresh) => fresh,
                None => {
                    // the daemon already booked the failed resolve
                    println!("item {item} part {part}: refresh failed");
                    return None;
                }
            };
            match probe(&opts.ffprobe, &fresh.url, &opts.user_agent).await {
                Ok(probed) => {
                    println!("item {item} part {part}: healed a dead link");
                    Some((probed, fresh))
                }
                Err(e) => {
                    report(target, opts, false).await;
                    // ffprobe errors can echo the signed URL; redact it
                    let e = redact_urls(&e.to_string());
                    println!("item {item} part {part}: still dead after refresh: {e}");
                    None
                }
            }
        }
        Err(e) => {
            // not a dead link, so no refresh — but it still failed, and
            // leaving it unbooked re-probes it on every later scan
            report(target, opts, false).await;
            // ffprobe errors can echo the signed URL; redact it
            let e = redact_urls(&e.to_string());
            println!("item {item} part {part}: probe: {e}");
            None
        }
    }
}

/// ffprobe reports upstream 4xx as "Server returned 4XX Client Error"
/// or "Server returned 403 Forbidden"; anything else (bad binary,
/// timeout, broken output) is not a dead link and must not refresh.
fn is_dead_link(e: &ProbeError) -> bool {
    matches!(e, ProbeError::Failed(msg)
        if msg.contains("Server returned 4") || msg.contains("HTTP error 4"))
}

/// Asks the daemon first (its persistent cache saves upstream
/// resolutions), falling back to resolving directly. A cached link past
/// its expiry is a guaranteed upstream failure, so it gets one eager
/// refresh; if that fails, the expired link is still the best we have
/// and the consumer's failure heal takes over from there.
///
/// A direct resolution that fails is booked with the daemon when one is
/// reachable, so a URL that cannot be parsed is not resolved again.
pub async fn resolve_link(target: &str, opts: &Options) -> Option<Link> {
    if let Some(sock) = &opts.runtime_sock
        && let Some(client) = shared_client(sock)
        && let Ok(res) = client.resolve(target).await
    {
        if res.expires_at == 0 || res.expires_at > now_secs() {
            return Some(link_of(res));
        }
        return refresh_link(target, opts)
            .await
            .or_else(|| Some(link_of(res)));
    }
    match linkres::resolve(target, &opts.user_agent).await {
        Ok(link) => Some(link),
        Err(err) => {
            report(target, opts, err.is_permanent()).await;
            None
        }
    }
}

/// The link a daemon answer carries.
fn link_of(res: Resolved) -> Link {
    Link {
        url: res.direct,
        expires_at: res.expires_at,
        ..Default::default()
    }
}

/// Wall-clock unix seconds; 0 on a clock before the epoch.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The scan-side heal: same shape as resolve_link but forcing a fresh
/// link, deduped per target by the caller.
async fn refresh_link(target: &str, opts: &Options) -> Option<Link> {
    if let Some(sock) = &opts.runtime_sock
        && let Some(client) = shared_client(sock)
        && let Ok(res) = client.refresh(target).await
    {
        return Some(link_of(res));
    }
    linkres::resolve(target, &opts.user_agent).await.ok()
}

/// Books a target with the daemon, when one is reachable. `permanent` is
/// skipped until a success clears it; anything else takes the growing
/// backoff.
async fn report(target: &str, opts: &Options, permanent: bool) {
    if let Some(sock) = &opts.runtime_sock
        && let Some(client) = shared_client(sock)
    {
        let _ = client.dead(target, permanent).await;
    }
}

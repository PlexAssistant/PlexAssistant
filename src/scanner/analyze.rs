use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};

use crate::linkres::{self, Link};
use crate::plexdb::{Db, WriteError, WriteSpec};
use crate::runtime::{Cache, Client, Identity};

use super::probe::{ProbeError, ProbeResult, probe};
use super::spec::build_spec;

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

/// Configures one analysis run.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub ffprobe: String,
    pub user_agent: String,
    /// daemon socket; None or unreachable = resolve directly
    pub runtime_sock: Option<PathBuf>,
    /// assistant cache; None falls back to `ASSISTANT_CACHE`, then the
    /// default path inside the container
    pub cache: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum AnalyzeError {
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error("item {item} part {part}: write: {source}")]
    Write {
        item: i64,
        part: i64,
        source: WriteError,
    },
}

/// Reports whether a part file is a .strm (case-insensitive).
pub fn is_strm(file: &str) -> bool {
    file.to_lowercase().ends_with(".strm")
}

/// One part whose probe succeeded and is ready to be written.
pub struct Probed {
    part: crate::plexdb::Part,
    spec: WriteSpec,
}

/// Resolves and probes every strm part of one item, without touching the
/// database. The library read happens first, so the rest can run alongside
/// other items; the caller writes the results back on its own connection.
/// Failures are logged and skipped. `refreshed` dedups link refreshes across
/// the whole run: every target gets at most one fresh link per round, which
/// still holds when several items are probed at once.
pub async fn probe_item(
    parts: Vec<crate::plexdb::Part>,
    item_id: i64,
    opts: &Options,
    refreshed: Arc<Mutex<HashSet<String>>>,
) -> Vec<Probed> {
    let mut out = Vec::new();
    for part in parts.into_iter().filter(|p| is_strm(&p.file)) {
        let target = match linkres::parse_strm(&part.file) {
            Ok(target) => target,
            Err(e) => {
                println!("item {item_id} part {}: {e}", part.id);
                continue;
            }
        };
        if !target.is_remote {
            continue; // local path inside a strm: not ours
        }
        // A part that already carries media info can be recognised before
        // any probe: duration and size are what the content ledger stores.
        // A hit means this file was probed before, under some other path or
        // item, and the rows can be copied instead of probed again.
        if let Some(spec) = reused_spec(&part, opts) {
            println!("item {item_id} part {}: same content, reusing", part.id);
            out.push(Probed { part, spec });
            continue;
        }
        let Some(link) = resolve_link(&target.value, opts).await else {
            println!("item {item_id} part {}: resolve failed", part.id);
            continue;
        };
        let Some((probed, link)) =
            probe_and_heal(&link, &target.value, opts, &refreshed, item_id, part.id).await
        else {
            continue;
        };
        let spec = remember_probe(&part, &probed, &link, opts);
        out.push(Probed { part, spec });
    }
    out
}

/// Turns a fresh probe into the row to write, and remembers it when the
/// probe has enough stable facts to recognise the file later.
///
/// The key is the probe, not the strm path or the signed link. A later
/// part of the same file — another path, a re-added item, a resigned URL —
/// finds the row and is written without a second probe only once that
/// lookup exists. The first sight of a file always probes: nothing stable
/// is known before ffprobe runs.
fn remember_probe(
    part: &crate::plexdb::Part,
    probed: &ProbeResult,
    link: &Link,
    opts: &Options,
) -> WriteSpec {
    let spec = build_spec(part, probed, link);
    if let Some(cache) = open_cache(opts)
        && let Some(identity) = Identity::from_probe(probed)
    {
        let _ = cache.remember_content(&identity.key(), &spec, std::time::SystemTime::now());
    }
    spec
}

/// The analysis already stored for a part whose library row already has
/// a duration and a size. Parts that have never been analysed return
/// nothing, and so does a cache that cannot be opened.
fn reused_spec(part: &crate::plexdb::Part, opts: &Options) -> Option<WriteSpec> {
    if part.duration <= 0 || part.size <= 0 {
        return None;
    }
    let cache = open_cache(opts)?;
    let stored = cache.content_for_media(part.duration, part.size).ok()??;
    Some(retarget(stored.spec, part))
}

/// Copies a remembered analysis onto another part of the same file.
fn retarget(mut spec: WriteSpec, part: &crate::plexdb::Part) -> WriteSpec {
    spec.media_item_id = part.media_item_id;
    spec.part_id = part.id;
    spec
}

fn open_cache(opts: &Options) -> Option<Cache> {
    let path = opts
        .cache
        .clone()
        .or_else(|| std::env::var("ASSISTANT_CACHE").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/config/assistant/cache.db"));
    Cache::open(path).ok()
}

/// Writes the probed parts of one item. A failed write stops the run.
pub fn write_item(db: &mut Db, item_id: i64, probed: Vec<Probed>) -> Result<usize, AnalyzeError> {
    for row in &probed {
        db.update_media(&row.spec)
            .map_err(|source| AnalyzeError::Write {
                item: item_id,
                part: row.part.id,
                source,
            })?;
    }
    Ok(probed.len())
}

/// Runs the pipeline for every strm part of one metadata item: resolve the
/// direct link, probe it, write the results. Non-strm parts and failures
/// are skipped without aborting the run; only a failed write stops it.
/// `refreshed` dedups link refreshes across the whole section run: every
/// target gets at most one fresh link per round, no matter how many parts
/// point at it.
pub async fn analyze_item(
    db: &mut Db,
    item_id: i64,
    opts: &Options,
    refreshed: &mut HashSet<String>,
) -> Result<usize, AnalyzeError> {
    let parts = db.parts_by_item(item_id)?;
    let shared = Arc::new(Mutex::new(std::mem::take(refreshed)));
    let probed = probe_item(parts, item_id, opts, shared.clone()).await;
    if let Ok(guard) = shared.lock() {
        *refreshed = guard.clone();
    }
    write_item(db, item_id, probed)
}

/// Probes the resolved link. A dead one (upstream 4xx) gets a single
/// deduped refresh per run — bounded by the number of distinct targets,
/// never by the number of items — and a target that stays dead is booked
/// into the ledger so later rounds back off instead of re-probing.
async fn probe_and_heal(
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
                report_dead(target, opts).await;
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
                    report_dead(target, opts).await;
                    // ffprobe errors can echo the signed URL; redact it
                    let e = super::redact_urls(&e.to_string());
                    println!("item {item} part {part}: still dead after refresh: {e}");
                    None
                }
            }
        }
        Err(e) => {
            // not a dead link, so no refresh — but it still failed, and
            // leaving it unbooked re-probes it on every later scan
            report_dead(target, opts).await;
            // ffprobe errors can echo the signed URL; redact it
            let e = super::redact_urls(&e.to_string());
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
            return Some(Link {
                url: res.direct,
                expires_at: res.expires_at,
                ..Default::default()
            });
        }
        return refresh_link(target, opts).await.or_else(|| {
            Some(Link {
                url: res.direct,
                expires_at: res.expires_at,
                ..Default::default()
            })
        });
    }
    match linkres::resolve(target, &opts.user_agent).await {
        Ok(link) => Some(link),
        Err(err) => {
            report(target, opts, err.is_permanent()).await;
            None
        }
    }
}

/// Wall-clock unix seconds; 0 on a clock before the epoch.
fn now_secs() -> i64 {
    std::time::SystemTime::now()
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
        return Some(Link {
            url: res.direct,
            expires_at: res.expires_at,
            ..Default::default()
        });
    }
    linkres::resolve(target, &opts.user_agent).await.ok()
}

/// Books a persistently dead target into the daemon's ledger.
async fn report_dead(target: &str, opts: &Options) {
    report(target, opts, false).await;
}

/// Books a target. `permanent` is skipped until a success clears it;
/// anything else takes the growing backoff.
async fn report(target: &str, opts: &Options, permanent: bool) {
    if let Some(sock) = &opts.runtime_sock
        && let Some(client) = shared_client(sock)
    {
        let booked = if permanent {
            client.dead_permanent(target).await
        } else {
            client.dead(target).await
        };
        let _ = booked;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{Cache, Daemon, Entry};
    use crate::scanner::probe::tests::{SAMPLE_PROBE_JSON, fake_ffprobe};
    use axum::Router;
    use axum::extract::Path as UrlPath;
    use axum::http::{StatusCode, header};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use rusqlite::Connection;
    use std::path::Path;
    use std::sync::Arc;

    /// A cloud drive whose /file/{id} redirects to a range-capable /dl.
    async fn cloud() -> String {
        let app = Router::new()
            .route(
                "/file/{id}",
                get(|UrlPath(id): UrlPath<String>| async move {
                    (StatusCode::FOUND, [(header::LOCATION, format!("/dl/{id}"))])
                }),
            )
            .route(
                "/dl/{id}",
                get(|| async {
                    (
                        StatusCode::PARTIAL_CONTENT,
                        [(header::CONTENT_RANGE, "bytes 0-0/10")],
                        "0",
                    )
                        .into_response()
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        base
    }

    /// Builds a Plex-like library where item 42 owns the given part files.
    fn library(dir: &Path, files: &[&Path]) -> Db {
        let path = dir.join("lib.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            r#"CREATE TABLE schema_migrations (version varchar(255) NOT NULL);
            INSERT INTO schema_migrations VALUES ('500000000001.231');
            CREATE TABLE metadata_items (id INTEGER PRIMARY KEY, duration INTEGER);
            INSERT INTO metadata_items VALUES (42, NULL);
            CREATE TABLE media_items (id INTEGER PRIMARY KEY, metadata_item_id INTEGER,
              library_section_id INTEGER,
              width INTEGER, height INTEGER, size INTEGER, duration INTEGER, bitrate INTEGER,
              container TEXT, video_codec TEXT, audio_codec TEXT, color_trc TEXT,
              display_aspect_ratio REAL, frames_per_second REAL, audio_channels INTEGER,
              display_offset INTEGER, media_analysis_version INTEGER, extra_data TEXT, updated_at INTEGER);
            CREATE TABLE media_parts (id INTEGER PRIMARY KEY, media_item_id INTEGER,
              file TEXT, size INTEGER DEFAULT 0, duration INTEGER, extra_data TEXT, updated_at INTEGER);
            CREATE TABLE media_streams (id INTEGER PRIMARY KEY AUTOINCREMENT,
              stream_type_id INTEGER, media_item_id INTEGER, media_part_id INTEGER,
              codec TEXT, language TEXT, "index" INTEGER, channels INTEGER, bitrate INTEGER,
              "default" INTEGER, forced INTEGER, extra_data TEXT, url TEXT,
              created_at INTEGER, updated_at INTEGER);"#,
        )
        .unwrap();
        for (i, file) in files.iter().enumerate() {
            let media_item = 100 + i as i64;
            conn.execute(
                "INSERT INTO media_items (id, metadata_item_id, library_section_id) VALUES (?, 42, 1)",
                [media_item],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO media_parts (id, media_item_id, file) VALUES (?, ?, ?)",
                rusqlite::params![1000 + i as i64, media_item, file.to_str().unwrap()],
            )
            .unwrap();
        }
        Db::open(path).unwrap()
    }

    fn written_items(dir: &Path) -> Vec<(i64, String)> {
        let conn = Connection::open(dir.join("lib.db")).unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, video_codec FROM media_items WHERE video_codec IS NOT NULL ORDER BY id",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[tokio::test]
    async fn analyzes_remote_strm_parts_only() {
        let base = cloud().await;
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, content: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, content).unwrap();
            path
        };
        let remote = write("ep1.STRM", &format!("{base}/file/ep1.mkv\n"));
        let local = write("ep2.strm", "/media/ep2.mkv\n");
        let empty = write("ep3.strm", "# nothing\n");
        let dead = write("ep4.strm", "http://127.0.0.1:1/file/ep4\n");
        let plain = write("ep5.mkv", "");
        let mut db = library(dir.path(), &[&remote, &local, &empty, &dead, &plain]);
        let opts = Options {
            ffprobe: fake_ffprobe(&dir, &format!("cat <<'JSON'\n{SAMPLE_PROBE_JSON}\nJSON")),
            user_agent: linkres::DEFAULT_UA.into(),
            runtime_sock: None,
            cache: None,
        };

        assert_eq!(
            analyze_item(&mut db, 42, &opts, &mut Default::default())
                .await
                .unwrap(),
            1
        );
        assert_eq!(written_items(dir.path()), [(100, "h264".into())]);
    }

    /// A part that already carries the duration and size of a remembered
    /// file is written from the ledger. ffprobe is not invoked.
    #[tokio::test]
    async fn known_content_skips_the_probe() {
        let dir = tempfile::tempdir().unwrap();
        let strm = dir.path().join("again.strm");
        std::fs::write(&strm, "http://127.0.0.1:1/file/again").unwrap();
        let mut db = library(dir.path(), &[&strm]);
        // the library row already knows what the first probe learned
        Connection::open(dir.path().join("lib.db"))
            .unwrap()
            .execute(
                "UPDATE media_parts SET size=3757806, duration=20016 WHERE id=1000",
                [],
            )
            .unwrap();

        let cache_path = dir.path().join("cache.db");
        let cache = Cache::open(&cache_path).unwrap();
        let probe = serde_json::from_str(SAMPLE_PROBE_JSON).unwrap();
        let part = crate::plexdb::Part {
            id: 1,
            media_item_id: 1,
            file: "/first.strm".into(),
            size: 3757806,
            duration: 20016,
        };
        let spec = crate::scanner::build_spec(
            &part,
            &probe,
            &crate::linkres::Link {
                size: 3757806,
                ..Default::default()
            },
        );
        let identity = crate::runtime::Identity::from_probe(&probe).unwrap();
        cache
            .remember_content(&identity.key(), &spec, std::time::SystemTime::now())
            .unwrap();
        drop(cache);

        let calls = dir.path().join("calls");
        let opts = Options {
            ffprobe: fake_ffprobe(
                &dir,
                &format!("echo probed >> '{}'\nexit 1", calls.display()),
            ),
            cache: Some(cache_path),
            ..Default::default()
        };
        assert_eq!(
            analyze_item(&mut db, 42, &opts, &mut Default::default())
                .await
                .unwrap(),
            1
        );
        assert!(!calls.exists(), "a known file must not be probed");
        assert_eq!(written_items(dir.path()), [(100, "h264".into())]);
    }

    #[tokio::test]
    async fn prefers_the_daemon_cache() {
        let dir = tempfile::tempdir().unwrap();
        // nothing listens upstream: only the daemon's cache can answer
        let target = "http://127.0.0.1:1/file/ep1";
        let strm = dir.path().join("ep1.strm");
        std::fs::write(&strm, target).unwrap();
        let mut db = library(dir.path(), &[&strm]);

        let cache = Cache::open(dir.path().join("cache.db")).unwrap();
        cache
            .put(&Entry {
                target: target.into(),
                direct: "http://cached/dl/ep1".into(),
                ..Default::default()
            })
            .unwrap();
        let sock = dir.path().join("assistant.sock");
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();
        let daemon = Daemon::new(Arc::new(cache), "", 30);
        tokio::spawn(async move { axum::serve(listener, daemon.router()).await.unwrap() });

        let probed_url = dir.path().join("probed");
        let opts = Options {
            ffprobe: fake_ffprobe(
                &dir,
                &format!(
                    "for a; do last=$a; done; echo \"$last\" > '{}'\ncat <<'JSON'\n{SAMPLE_PROBE_JSON}\nJSON",
                    probed_url.display()
                ),
            ),
            user_agent: linkres::DEFAULT_UA.into(),
            runtime_sock: Some(sock),
            cache: None,
        };

        assert_eq!(
            analyze_item(&mut db, 42, &opts, &mut Default::default())
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            std::fs::read_to_string(probed_url).unwrap().trim(),
            "http://cached/dl/ep1"
        );
    }

    /// A cached link past its expiry is never handed to the probe: it
    /// refreshes eagerly, so a credits run fired long after the scan
    /// that cached the link still sees a working one.
    #[tokio::test]
    async fn expired_cache_entry_refreshes_eagerly() {
        // upstream signs fresh links under /dl/fresh-*
        let app = Router::new()
            .route(
                "/file/{id}",
                get(|UrlPath(id): UrlPath<String>| async move {
                    (
                        StatusCode::FOUND,
                        [(header::LOCATION, format!("/dl/fresh-{id}"))],
                    )
                }),
            )
            .route(
                "/dl/{id}",
                get(|| async {
                    (
                        StatusCode::PARTIAL_CONTENT,
                        [(header::CONTENT_RANGE, "bytes 0-0/10")],
                        "0",
                    )
                        .into_response()
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let dir = tempfile::tempdir().unwrap();
        let target = format!("{base}/file/ep1");
        let strm = dir.path().join("ep1.strm");
        std::fs::write(&strm, &target).unwrap();
        let mut db = library(dir.path(), &[&strm]);

        // the cache holds a link that expired long ago
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();
        cache
            .put(&Entry {
                target: target.clone(),
                direct: format!("{base}/dl/stale"),
                expires_at: 1000,
                ..Default::default()
            })
            .unwrap();
        let sock = dir.path().join("assistant.sock");
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();
        let daemon = Daemon::new(Arc::new(cache), "", 30);
        tokio::spawn(async move { axum::serve(listener, daemon.router()).await.unwrap() });

        let probed_url = dir.path().join("probed");
        let opts = Options {
            ffprobe: fake_ffprobe(
                &dir,
                &format!(
                    "for a; do last=$a; done; echo \"$last\" > '{}'\ncat <<'JSON'\n{SAMPLE_PROBE_JSON}\nJSON",
                    probed_url.display()
                ),
            ),
            user_agent: linkres::DEFAULT_UA.into(),
            runtime_sock: Some(sock),
            cache: None,
        };

        assert_eq!(
            analyze_item(&mut db, 42, &opts, &mut Default::default())
                .await
                .unwrap(),
            1
        );
        // the stale link never reached ffprobe
        assert_eq!(
            std::fs::read_to_string(probed_url).unwrap().trim(),
            format!("{base}/dl/fresh-ep1")
        );
    }

    #[tokio::test]
    async fn probe_failure_skips_the_part() {
        let base = cloud().await;
        let dir = tempfile::tempdir().unwrap();
        let strm = dir.path().join("ep1.strm");
        std::fs::write(&strm, format!("{base}/file/ep1")).unwrap();
        let mut db = library(dir.path(), &[&strm]);
        let opts = Options {
            ffprobe: fake_ffprobe(&dir, "echo boom >&2; exit 1"),
            cache: None,
            ..Default::default()
        };
        assert_eq!(
            analyze_item(&mut db, 42, &opts, &mut Default::default())
                .await
                .unwrap(),
            0
        );
        assert!(written_items(dir.path()).is_empty());
    }

    /// A cached dead link heals within the same run: one deduped refresh
    /// per target, however many parts point at it.
    #[tokio::test]
    async fn dead_cache_link_heals_within_the_run() {
        // upstream signs fresh links under /dl/fresh-*
        let app = Router::new()
            .route(
                "/file/{id}",
                get(|UrlPath(id): UrlPath<String>| async move {
                    (
                        StatusCode::FOUND,
                        [(header::LOCATION, format!("/dl/fresh-{id}"))],
                    )
                }),
            )
            .route(
                "/dl/{id}",
                get(|| async {
                    (
                        StatusCode::PARTIAL_CONTENT,
                        [(header::CONTENT_RANGE, "bytes 0-0/10")],
                        "0",
                    )
                        .into_response()
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let dir = tempfile::tempdir().unwrap();
        // ffprobe answers only fresh links; a stale one reports upstream 4xx
        let ffprobe = fake_ffprobe(
            &dir,
            &format!(
                "for a; do case \"$a\" in *fresh*) cat <<'JSON'\n{SAMPLE_PROBE_JSON}\nJSON\nexit 0;; esac; done\n\
                 echo 'Server returned 4XX Client Error' >&2; exit 1"
            ),
        );
        let target = format!("{base}/file/ep1");
        let s1 = dir.path().join("a.strm");
        let s2 = dir.path().join("b.strm");
        std::fs::write(&s1, &target).unwrap();
        std::fs::write(&s2, &target).unwrap();
        let mut db = library(dir.path(), &[&s1, &s2]);

        // the daemon's cache holds a stale link for the target
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();
        cache
            .put(&Entry {
                target: target.clone(),
                direct: format!("{base}/dl/stale"),
                ..Default::default()
            })
            .unwrap();
        let sock = dir.path().join("assistant.sock");
        let dlistener = tokio::net::UnixListener::bind(&sock).unwrap();
        let daemon = Daemon::new(Arc::new(cache), "", 30);
        tokio::spawn(async move { axum::serve(dlistener, daemon.router()).await.unwrap() });

        let opts = Options {
            ffprobe,
            runtime_sock: Some(sock),
            cache: None,
            ..Default::default()
        };
        let mut refreshed = HashSet::new();
        assert_eq!(
            analyze_item(&mut db, 42, &opts, &mut refreshed)
                .await
                .unwrap(),
            2
        );
        assert_eq!(refreshed.len(), 1, "one refresh per target");
        assert_eq!(written_items(dir.path()).len(), 2);
    }

    #[tokio::test]
    async fn write_failure_aborts() {
        let base = cloud().await;
        let dir = tempfile::tempdir().unwrap();
        let strm = dir.path().join("ep1.strm");
        std::fs::write(&strm, format!("{base}/file/ep1")).unwrap();
        let mut db = library(dir.path(), &[&strm]);
        Connection::open(dir.path().join("lib.db"))
            .unwrap()
            .execute("UPDATE schema_migrations SET version='9'", [])
            .unwrap();
        let opts = Options {
            ffprobe: fake_ffprobe(&dir, &format!("cat <<'JSON'\n{SAMPLE_PROBE_JSON}\nJSON")),
            cache: None,
            ..Default::default()
        };
        let err = analyze_item(&mut db, 42, &opts, &mut Default::default())
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "item 42 part 1000: write: schema \"9\" not on the write whitelist"
        );
    }

    #[test]
    fn strm_suffix_is_case_insensitive() {
        assert!(is_strm("/a/B.STRM"));
        assert!(is_strm("x.strm"));
        assert!(!is_strm("x.mkv"));
        assert!(!is_strm("strm"));
    }
}

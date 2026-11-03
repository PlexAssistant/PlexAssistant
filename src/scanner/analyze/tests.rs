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
        analyze_item(&mut db, 42, &opts, &Refreshed::default())
            .await
            .unwrap(),
        1
    );
    assert_eq!(written_items(dir.path()), [(100, "h264".into())]);
}

/// A probe that fails books the part. A permanent booking is not tried
/// again; a temporary one is, and a later success clears the row.
#[tokio::test]
async fn a_failed_part_is_booked() {
    let base = cloud().await;
    let dir = tempfile::tempdir().unwrap();
    let strm = dir.path().join("ep1.strm");
    std::fs::write(&strm, format!("{base}/file/ep1")).unwrap();
    let mut db = library(dir.path(), &[&strm]);
    let cache_path = dir.path().join("cache.db");
    let opts = Options {
        ffprobe: fake_ffprobe(&dir, "echo boom >&2; exit 1"),
        cache: Some(cache_path.clone()),
        ..Default::default()
    };
    assert_eq!(
        analyze_item(&mut db, 42, &opts, &Refreshed::default())
            .await
            .unwrap(),
        0
    );
    let cache = Cache::open(&cache_path).unwrap();
    let row = cache.part_failure(1000).unwrap().unwrap();
    assert_eq!(row.attempts, 1);
    assert_eq!(row.kind, crate::runtime::PartFailure::Temporary);
    drop(cache);

    // the same part, now booked permanent, is not probed again
    Cache::open(&cache_path)
        .unwrap()
        .record_part_failure(
            1000,
            crate::runtime::PartFailure::Permanent,
            std::time::SystemTime::now(),
        )
        .unwrap();
    let calls = dir.path().join("calls");
    let opts = Options {
        ffprobe: fake_ffprobe(
            &dir,
            &format!("echo probed >> '{}'; exit 1", calls.display()),
        ),
        cache: Some(cache_path),
        ..Default::default()
    };
    assert_eq!(
        analyze_item(&mut db, 42, &opts, &Refreshed::default())
            .await
            .unwrap(),
        0
    );
    assert!(!calls.exists(), "a permanent part must not be probed");
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
        analyze_item(&mut db, 42, &opts, &Refreshed::default())
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
        analyze_item(&mut db, 42, &opts, &Refreshed::default())
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
        analyze_item(&mut db, 42, &opts, &Refreshed::default())
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
        analyze_item(&mut db, 42, &opts, &Refreshed::default())
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
    let refreshed = Refreshed::default();
    assert_eq!(
        analyze_item(&mut db, 42, &opts, &refreshed).await.unwrap(),
        2
    );
    assert_eq!(refreshed.lock().unwrap().len(), 1, "one refresh per target");
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
    let err = analyze_item(&mut db, 42, &opts, &Refreshed::default())
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

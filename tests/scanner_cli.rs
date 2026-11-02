//! Drives the built binary through a "Plex Media Scanner" symlink, the
//! way Plex invokes it: unhandled modes must exec the preserved original,
//! strm analysis must run our pipeline and write the library.

use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Output;

use axum::Router;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use rusqlite::Connection;

const PROBE_JSON: &str = r#"{"streams":[{"index":0,"codec_name":"h264","codec_type":"video",
  "width":1280,"height":720,"avg_frame_rate":"24/1"}],
  "format":{"format_name":"matroska,webm","duration":"20.0","size":"1000","bit_rate":"400"}}"#;

struct Rig {
    dir: tempfile::TempDir,
    scanner: PathBuf,
}

impl Rig {
    /// A plex dir whose scanner is our binary, with a fake original that
    /// echoes its argv.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let scanner = dir.path().join("Plex Media Scanner");
        symlink(env!("CARGO_BIN_EXE_assistant"), &scanner).unwrap();
        script(
            &dir.path().join("Plex Media Scanner.real"),
            r#"echo "original: $0 $*""#,
        );
        script(
            &dir.path().join("ffprobe"),
            &format!("cat <<'JSON'\n{PROBE_JSON}\nJSON"),
        );
        Self { dir, scanner }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// Item 42 in section 1 owns one part with the given file.
    fn library(&self, file: &Path) -> PathBuf {
        let db = self.path("lib.db");
        let conn = Connection::open(&db).unwrap();
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
              display_offset INTEGER, media_analysis_version INTEGER, updated_at INTEGER);
            INSERT INTO media_items (id, metadata_item_id, library_section_id) VALUES (7, 42, 1);
            CREATE TABLE media_parts (id INTEGER PRIMARY KEY, media_item_id INTEGER,
              file TEXT, size INTEGER DEFAULT 0, duration INTEGER, updated_at INTEGER);
            CREATE TABLE media_streams (id INTEGER PRIMARY KEY AUTOINCREMENT,
              stream_type_id INTEGER, media_item_id INTEGER, media_part_id INTEGER,
              codec TEXT, language TEXT, "index" INTEGER, channels INTEGER, bitrate INTEGER,
              "default" INTEGER, forced INTEGER, extra_data TEXT, url TEXT,
              created_at INTEGER, updated_at INTEGER);"#,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_parts (id, media_item_id, file) VALUES (70, 7, ?)",
            [file.to_str().unwrap()],
        )
        .unwrap();
        db
    }

    async fn run(&self, args: &[&str]) -> Output {
        tokio::process::Command::new(&self.scanner)
            .args(args)
            .env("ASSISTANT_FFPROBE", self.path("ffprobe"))
            .output()
            .await
            .unwrap()
    }
}

fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

async fn cloud() -> String {
    let app = Router::new()
        .route(
            "/file/ep1",
            get(|| async { (StatusCode::FOUND, [(header::LOCATION, "/dl/ep1")]) }),
        )
        .route(
            "/dl/ep1",
            get(|| async {
                (
                    StatusCode::PARTIAL_CONTENT,
                    [(header::CONTENT_RANGE, "bytes 0-0/1000")],
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

#[tokio::test]
async fn unhandled_modes_exec_the_original() {
    let rig = Rig::new();
    let out = rig.run(&["--scan", "--refresh", "--section", "1"]).await;
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        stdout(&out).trim(),
        format!(
            "original: {} --scan --refresh --section 1",
            rig.path("Plex Media Scanner.real").display()
        )
    );
}

#[tokio::test]
async fn local_media_goes_to_the_original() {
    let rig = Rig::new();
    let db = rig.library(Path::new("/media/movie.mkv"));
    let db = db.to_str().unwrap();
    let out = rig
        .run(&["--analyze", "--item", "42", "--library-db", db])
        .await;
    assert!(stdout(&out).starts_with("original: "), "{out:?}");
    assert!(stdout(&out).contains("--analyze --item 42"));
}

#[tokio::test]
async fn analyzes_strm_items() {
    let base = cloud().await;
    let rig = Rig::new();
    let strm = rig.path("ep1.strm");
    std::fs::write(&strm, format!("{base}/file/ep1\n")).unwrap();
    let db = rig.library(&strm);
    let out = rig
        .run(&[
            "--analyze",
            "--log-file-suffix",
            " Analysis",
            "--item",
            "42",
            "--library-db",
            db.to_str().unwrap(),
            "--runtime-sock",
            "",
        ])
        .await;
    assert!(out.status.success(), "{out:?}");
    assert_eq!(stdout(&out).trim(), "item 42: analyzed 1 strm part(s)");

    let codec: String = Connection::open(&db)
        .unwrap()
        .query_row("SELECT video_codec FROM media_items WHERE id=7", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(codec, "h264");
}

#[tokio::test]
async fn analyzes_sections() {
    let base = cloud().await;
    let rig = Rig::new();
    let strm = rig.path("ep1.strm");
    std::fs::write(&strm, format!("{base}/file/ep1\n")).unwrap();
    let db = rig.library(&strm);
    let out = rig
        .run(&[
            "--analyze",
            "--section",
            "1",
            "--library-db",
            db.to_str().unwrap(),
        ])
        .await;
    assert!(out.status.success(), "{out:?}");
    assert_eq!(stdout(&out).trim(), "section 1: 1 item(s) processed");
}

#[tokio::test]
async fn missing_original_is_an_error() {
    let rig = Rig::new();
    std::fs::remove_file(rig.path("Plex Media Scanner.real")).unwrap();
    let out = rig.run(&["--scan"]).await;
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("original scanner not found"),
        "{out:?}"
    );
}

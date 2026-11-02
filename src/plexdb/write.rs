use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::params;

use super::Db;

/// One media_streams row to write.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StreamRow {
    /// 1 video, 2 audio, 3 subtitle
    pub type_id: i64,
    pub codec: String,
    pub language: String,
    pub index: i64,
    pub channels: i64,
    pub bitrate: i64,
    /// JSON; ma:profile drives the client's stream label
    pub extra_data: String,
}

/// The analyzed media info for one part, shaped exactly like the rows the
/// official scanner writes (measured against a live Plex).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WriteSpec {
    pub media_item_id: i64,
    pub part_id: i64,
    pub width: i64,
    pub height: i64,
    pub size: i64,
    pub duration_ms: i64,
    pub bitrate: i64,
    pub container: String,
    pub video_codec: String,
    pub audio_codec: String,
    /// e.g. smpte2084; empty for SDR
    pub color_trc: String,
    pub aspect_ratio: f64,
    pub fps: f64,
    pub audio_channels: i64,
    pub streams: Vec<StreamRow>,
}

#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("schema {0:?} not on the write whitelist")]
    UnknownSchema(String),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

impl Db {
    /// Makes an online snapshot of the database via VACUUM INTO.
    pub fn backup(&self, dst: impl AsRef<Path>) -> rusqlite::Result<()> {
        let dst = dst.as_ref().to_string_lossy();
        self.conn.execute("VACUUM INTO ?", [dst.as_ref()])?;
        Ok(())
    }

    /// Writes the spec inside one transaction: refreshes media_items, the
    /// part, replaces the streams, and syncs metadata_items.duration when
    /// it is still unknown. It refuses to run against a schema that is
    /// not whitelisted.
    pub fn update_media(&mut self, spec: &WriteSpec) -> Result<(), WriteError> {
        let (known, version) = self.schema_known()?;
        if !known {
            return Err(WriteError::UnknownSchema(version));
        }

        let tx = self.conn.transaction()?;
        let now = unix_now();
        tx.execute(
            "UPDATE media_items SET
                width=?, height=?, size=?, duration=?, bitrate=?,
                container=?, video_codec=?, audio_codec=?, color_trc=?,
                display_aspect_ratio=?, frames_per_second=?, audio_channels=?,
                display_offset=0, media_analysis_version=8, updated_at=?
             WHERE id=?",
            params![
                spec.width,
                spec.height,
                spec.size,
                spec.duration_ms,
                spec.bitrate,
                spec.container,
                spec.video_codec,
                spec.audio_codec,
                spec.color_trc,
                spec.aspect_ratio,
                spec.fps,
                spec.audio_channels,
                now,
                spec.media_item_id,
            ],
        )?;
        tx.execute(
            "UPDATE media_parts SET size=?, duration=?, updated_at=? WHERE id=?",
            params![spec.size, spec.duration_ms, now, spec.part_id],
        )?;
        tx.execute(
            "DELETE FROM media_streams WHERE media_item_id=?",
            [spec.media_item_id],
        )?;
        {
            let mut insert = tx.prepare(
                r#"INSERT INTO media_streams
                    (stream_type_id, media_item_id, media_part_id, codec, language,
                     "index", channels, bitrate, "default", forced, extra_data, updated_at)
                   VALUES (?,?,?,?,?,?,?,?,0,0,?,?)"#,
            )?;
            for s in &spec.streams {
                // NULL when unknown, like the official rows
                let lang = Some(s.language.as_str()).filter(|l| !l.is_empty() && *l != "und");
                insert.execute(params![
                    s.type_id,
                    spec.media_item_id,
                    spec.part_id,
                    s.codec,
                    lang,
                    s.index,
                    Some(s.channels).filter(|&c| c > 0),
                    Some(s.bitrate).filter(|&b| b > 0),
                    Some(s.extra_data.as_str()).filter(|e| !e.is_empty()),
                    now,
                ])?;
            }
        }
        tx.execute(
            "UPDATE metadata_items SET duration=?
             WHERE id=(SELECT metadata_item_id FROM media_items WHERE id=?)
               AND (duration IS NULL OR duration=0)",
            params![spec.duration_ms, spec.media_item_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Truncates the WAL after a batch of writes.
    pub fn checkpoint(&self) -> rusqlite::Result<()> {
        // the pragma reports (busy, log, checkpointed) as a row
        self.conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::types::Value;

    /// Builds a temp database with write-relevant tables and one item
    /// carrying a part.
    fn fixture() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("lib.db")).unwrap();
        db.conn
            .execute_batch(
                r#"CREATE TABLE schema_migrations (version varchar(255) NOT NULL);
                INSERT INTO schema_migrations VALUES ('500000000001.231');
                CREATE TABLE metadata_items (id INTEGER PRIMARY KEY, duration INTEGER);
                INSERT INTO metadata_items VALUES (42, NULL);
                CREATE TABLE media_items (id INTEGER PRIMARY KEY, metadata_item_id INTEGER,
                  width INTEGER, height INTEGER, size INTEGER, duration INTEGER, bitrate INTEGER,
                  container TEXT, video_codec TEXT, audio_codec TEXT, color_trc TEXT,
                  display_aspect_ratio REAL, frames_per_second REAL, audio_channels INTEGER,
                  display_offset INTEGER, media_analysis_version INTEGER, updated_at INTEGER);
                INSERT INTO media_items (id, metadata_item_id) VALUES (7, 42);
                CREATE TABLE media_parts (id INTEGER PRIMARY KEY, media_item_id INTEGER,
                  size INTEGER, duration INTEGER, updated_at INTEGER);
                INSERT INTO media_parts (id, media_item_id, size, duration) VALUES (70, 7, 35, NULL);
                CREATE TABLE media_streams (id INTEGER PRIMARY KEY AUTOINCREMENT,
                  stream_type_id INTEGER, media_item_id INTEGER, media_part_id INTEGER,
                  codec TEXT, language TEXT, "index" INTEGER, channels INTEGER, bitrate INTEGER,
                  "default" INTEGER, forced INTEGER, extra_data TEXT, updated_at INTEGER);"#,
            )
            .unwrap();
        (dir, db)
    }

    fn sample_spec() -> WriteSpec {
        WriteSpec {
            media_item_id: 7,
            part_id: 70,
            width: 1280,
            height: 720,
            size: 3757806,
            duration_ms: 20023,
            bitrate: 1501395,
            container: "mkv".into(),
            video_codec: "h264".into(),
            audio_codec: "aac".into(),
            color_trc: "smpte2084".into(),
            aspect_ratio: 1.7777777910232544,
            fps: 24.0,
            audio_channels: 1,
            streams: vec![
                StreamRow {
                    type_id: 1,
                    codec: "h264".into(),
                    index: 0,
                    bitrate: 1501395,
                    extra_data: r#"{"ma:profile":"high"}"#.into(),
                    ..Default::default()
                },
                StreamRow {
                    type_id: 2,
                    codec: "aac".into(),
                    index: 1,
                    language: "und".into(),
                    channels: 2,
                    bitrate: 96000,
                    ..Default::default()
                },
            ],
        }
    }

    fn value(db: &Db, sql: &str) -> Value {
        db.conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn update_media() {
        let (_dir, mut db) = fixture();
        db.update_media(&sample_spec()).unwrap();

        let item: (i64, i64, i64, String, String) = db
            .conn
            .query_row(
                "SELECT width, height, duration, container, color_trc FROM media_items WHERE id=7",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(item, (1280, 720, 20023, "mkv".into(), "smpte2084".into()));

        let part: (i64, i64) = db
            .conn
            .query_row(
                "SELECT size, duration FROM media_parts WHERE id=70",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(part, (3757806, 20023));

        let video = "FROM media_streams WHERE media_item_id=7 AND stream_type_id=1";
        let audio = "FROM media_streams WHERE media_item_id=7 AND stream_type_id=2";
        assert_eq!(
            value(&db, &format!("SELECT count(*) {video}")),
            Value::Integer(1)
        );
        assert_eq!(
            value(&db, &format!("SELECT bitrate {video}")),
            Value::Integer(1501395)
        );
        assert_eq!(
            value(&db, &format!("SELECT channels {audio}")),
            Value::Integer(2)
        );
        // unknown values are stored as NULL
        assert_eq!(value(&db, &format!("SELECT channels {video}")), Value::Null);
        assert_eq!(value(&db, &format!("SELECT language {audio}")), Value::Null);
        assert_eq!(
            value(&db, &format!("SELECT extra_data {audio}")),
            Value::Null
        );
        assert_eq!(
            value(&db, &format!("SELECT extra_data {video}")),
            Value::Text(r#"{"ma:profile":"high"}"#.into())
        );
        assert_eq!(
            value(&db, "SELECT duration FROM metadata_items WHERE id=42"),
            Value::Integer(20023)
        );
    }

    #[test]
    fn keeps_known_metadata_duration() {
        let (_dir, mut db) = fixture();
        db.conn
            .execute("UPDATE metadata_items SET duration=999 WHERE id=42", [])
            .unwrap();
        db.update_media(&sample_spec()).unwrap();
        assert_eq!(
            value(&db, "SELECT duration FROM metadata_items WHERE id=42"),
            Value::Integer(999)
        );
    }

    #[test]
    fn replaces_streams() {
        let (_dir, mut db) = fixture();
        db.update_media(&sample_spec()).unwrap();
        let mut spec = sample_spec();
        spec.streams.truncate(1); // second write: only video
        db.update_media(&spec).unwrap();
        assert_eq!(
            value(
                &db,
                "SELECT count(*) FROM media_streams WHERE media_item_id=7"
            ),
            Value::Integer(1)
        );
    }

    #[test]
    fn refuses_unknown_schema() {
        let (_dir, mut db) = fixture();
        db.conn
            .execute("UPDATE schema_migrations SET version='9'", [])
            .unwrap();
        let err = db.update_media(&sample_spec()).unwrap_err();
        assert!(
            matches!(&err, WriteError::UnknownSchema(v) if v == "9"),
            "{err:?}"
        );
        assert_eq!(
            value(&db, "SELECT count(*) FROM media_streams"),
            Value::Integer(0)
        );
    }

    #[test]
    fn backup_and_checkpoint() {
        let (dir, db) = fixture();
        // a quote in the path must not break the statement
        let dst = dir.path().join("it's a backup.db");
        db.backup(&dst).unwrap();
        let bk = Db::open(&dst).unwrap();
        assert!(bk.schema_known().unwrap().0, "backup unreadable");
        db.checkpoint().unwrap();
    }
}

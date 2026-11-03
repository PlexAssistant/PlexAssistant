//! Reads and writes the Plex library SQLite database directly, with the
//! write-safety protocol (busy_timeout, schema gate, backup before
//! writes).

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, Result};

mod markers;
mod tracks;
mod write;

pub use markers::{MARKER_CREDITS, MARKER_INTRO};
pub use tracks::{SavedSelection, TrackChoice};
pub use write::{StreamRow, WriteError, WriteSpec};

/// The library database inside the Plex container.
pub const DEFAULT_PATH: &str = "/config/Library/Application Support/Plex Media Server/Plug-in Support/Databases/com.plexapp.plugins.library.db";

/// schema_migrations major versions this build can handle. Minor
/// differences are tolerated: Plex ships bugfix migrations that do not
/// move the tables we touch. An unknown major means "refuse to write"
/// (reads stay allowed).
const KNOWN_SCHEMA_MAJORS: &[&str] = &[
    "500000000001", // known-good with Plex 1.43.5
];

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// A handle on one Plex library database.
pub struct Db {
    conn: Connection,
}

/// One media part (typically one file) of a metadata item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub id: i64,
    pub media_item_id: i64,
    pub file: String,
    pub size: i64,
    /// ms, 0 = unknown
    pub duration: i64,
}

impl Part {
    /// Duration and size, the pair the content and marker ledgers match
    /// on. Empty for a part that has not been analysed yet, so nothing
    /// matches it and detection runs.
    pub fn media_key(&self) -> String {
        if self.duration <= 0 || self.size <= 0 {
            return String::new();
        }
        format!("d={}\ns={}\n", self.duration, self.size)
    }
}

impl Db {
    /// Opens the database with the write-safety busy timeout applied.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        Ok(Self { conn })
    }

    /// Returns schema_migrations.max(version), empty when the table is
    /// empty.
    pub fn schema_version(&self) -> Result<String> {
        let v: Option<String> =
            self.conn
                .query_row("SELECT max(version) FROM schema_migrations", [], |r| {
                    r.get(0)
                })?;
        Ok(v.unwrap_or_default())
    }

    /// Reports whether the database schema is on the whitelist, along with
    /// the version found.
    pub fn schema_known(&self) -> Result<(bool, String)> {
        let v = self.schema_version()?;
        let major = v.split('.').next().unwrap_or_default();
        Ok((KNOWN_SCHEMA_MAJORS.contains(&major), v))
    }

    /// Returns the media parts of a metadata item, joined through
    /// media_items.
    pub fn parts_by_item(&self, item_id: i64) -> Result<Vec<Part>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT p.id, p.media_item_id, p.file, p.size, IFNULL(p.duration, 0)
             FROM media_parts p
             JOIN media_items mi ON mi.id = p.media_item_id
             WHERE mi.metadata_item_id = ?
             ORDER BY p.id",
        )?;
        stmt.query_map([item_id], |r| {
            Ok(Part {
                id: r.get(0)?,
                media_item_id: r.get(1)?,
                file: r.get(2)?,
                size: r.get(3)?,
                duration: r.get(4)?,
            })
        })?
        .collect()
    }

    /// Returns the metadata item ids of a library section.
    pub fn items_by_section(&self, section_id: i64) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT metadata_item_id FROM media_items WHERE library_section_id=?",
        )?;
        stmt.query_map([section_id], |r| r.get(0))?.collect()
    }

    /// Returns the strm parts of every episode in a season, keyed by
    /// their metadata item id. metadata_items hierarchy: show(2) →
    /// season(3) → episode(4, parent_id = season id).
    pub fn strm_episodes_by_season(&self, season_id: i64) -> Result<Vec<(i64, Part)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT mi.metadata_item_id, p.id, p.media_item_id, p.file, p.size,
                    IFNULL(p.duration, 0)
             FROM metadata_items ep
             JOIN media_items mi ON mi.metadata_item_id = ep.id
             JOIN media_parts p ON p.media_item_id = mi.id
             WHERE ep.parent_id = ? AND ep.metadata_type = 4
               AND LOWER(p.file) LIKE '%.strm'
             ORDER BY ep.[index]",
        )?;
        stmt.query_map([season_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                Part {
                    id: r.get(1)?,
                    media_item_id: r.get(2)?,
                    file: r.get(3)?,
                    size: r.get(4)?,
                    duration: r.get(5)?,
                },
            ))
        })?
        .collect()
    }

    /// The season an episode belongs to, when the item is an episode.
    pub fn season_of_item(&self, item_id: i64) -> Result<Option<i64>> {
        let season = self.conn.query_row(
            "SELECT parent_id FROM metadata_items WHERE id=? AND metadata_type=4",
            [item_id],
            |row| row.get(0),
        );
        match season {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Returns the ids of the seasons in a library section that have at
    /// least one episode with media.
    pub fn seasons_by_section(&self, section_id: i64) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT ep.parent_id
             FROM media_items mi
             JOIN metadata_items ep ON ep.id = mi.metadata_item_id
             WHERE mi.library_section_id = ? AND ep.metadata_type = 4
               AND ep.parent_id IS NOT NULL
             ORDER BY ep.parent_id",
        )?;
        stmt.query_map([section_id], |r| r.get(0))?.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a temp database with the minimal Plex-like schema and one
    /// item carrying two parts.
    fn fixture(schema_version: &str) -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("lib.db")).unwrap();
        db.conn
            .execute_batch(
                "CREATE TABLE schema_migrations (version varchar(255) NOT NULL);
                 CREATE TABLE media_items (id INTEGER PRIMARY KEY, metadata_item_id INTEGER,
                   library_section_id INTEGER);
                 CREATE TABLE media_parts (id INTEGER PRIMARY KEY, media_item_id INTEGER,
                   file TEXT, size INTEGER DEFAULT 0, duration INTEGER);
                 INSERT INTO media_items VALUES (1, 42, 1);
                 INSERT INTO media_parts VALUES (10, 1, '/data/a.strm', 35, NULL);
                 INSERT INTO media_parts VALUES (11, 1, '/data/b.strm', 48, 1200);",
            )
            .unwrap();
        db.conn
            .execute("INSERT INTO schema_migrations VALUES (?)", [schema_version])
            .unwrap();
        (dir, db)
    }

    #[test]
    fn schema_known() {
        let (_dir, db) = fixture("500000000001.231");
        assert_eq!(
            db.schema_known().unwrap(),
            (true, "500000000001.231".into())
        );

        // minor differences are tolerated: same tables, newer migrations
        let (_dir, db) = fixture("500000000001.232");
        assert_eq!(
            db.schema_known().unwrap(),
            (true, "500000000001.232".into())
        );

        // a different major means untouched territory: refuse to write
        let (_dir, db) = fixture("999999999999.999");
        assert_eq!(
            db.schema_known().unwrap(),
            (false, "999999999999.999".into())
        );
    }

    #[test]
    fn empty_schema_table() {
        let (_dir, db) = fixture("x");
        db.conn
            .execute("DELETE FROM schema_migrations", [])
            .unwrap();
        assert_eq!(db.schema_known().unwrap(), (false, String::new()));
    }

    #[test]
    fn parts_by_item() {
        let (_dir, db) = fixture("500000000001.231");
        let parts = db.parts_by_item(42).unwrap();
        assert_eq!(
            parts,
            [
                Part {
                    id: 10,
                    media_item_id: 1,
                    file: "/data/a.strm".into(),
                    size: 35,
                    duration: 0,
                },
                Part {
                    id: 11,
                    media_item_id: 1,
                    file: "/data/b.strm".into(),
                    size: 48,
                    duration: 1200,
                },
            ]
        );
        assert!(db.parts_by_item(999).unwrap().is_empty());
    }

    #[test]
    fn seasons_by_section_stays_in_the_section() {
        let (_dir, db) = fixture("500000000001.231");
        db.conn
            .execute_batch(
                "CREATE TABLE metadata_items (id INTEGER PRIMARY KEY, parent_id INTEGER,
                   metadata_type INTEGER);
                 INSERT INTO metadata_items VALUES (42, 7, 4);
                 INSERT INTO metadata_items VALUES (43, 8, 4);
                 INSERT INTO metadata_items VALUES (44, NULL, 1);
                 INSERT INTO media_items VALUES (2, 43, 2);
                 INSERT INTO media_items VALUES (3, 44, 2);",
            )
            .unwrap();
        assert_eq!(db.seasons_by_section(1).unwrap(), [7]);
        // a movie in the same section has no season
        assert_eq!(db.seasons_by_section(2).unwrap(), [8]);
        assert!(db.seasons_by_section(3).unwrap().is_empty());
    }

    #[test]
    fn media_key_needs_duration_and_size() {
        let (_dir, db) = fixture("500000000001.231");
        let parts = db.parts_by_item(42).unwrap();
        assert_eq!(parts[0].media_key(), "", "no duration yet");
        assert_eq!(parts[1].media_key(), "d=1200\ns=48\n");
    }

    #[test]
    fn items_by_section() {
        let (_dir, db) = fixture("500000000001.231");
        assert_eq!(db.items_by_section(1).unwrap(), [42]);
        assert!(db.items_by_section(99).unwrap().is_empty());
    }
}

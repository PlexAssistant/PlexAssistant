//! Reads and writes the Plex library SQLite database directly. The safety
//! protocol (busy_timeout, schema whitelist, backup before writes) follows
//! the measured behavior in development-plan §8.

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, Result};

mod markers;
mod write;

pub use markers::{MARKER_CREDITS, MARKER_INTRO};
pub use write::{StreamRow, WriteError, WriteSpec};

/// The library database inside the Plex container.
pub const DEFAULT_PATH: &str = "/config/Library/Application Support/Plex Media Server/Plug-in Support/Databases/com.plexapp.plugins.library.db";

/// schema_migrations versions this build can handle; an unknown version
/// means "refuse to write" (reads stay allowed).
const KNOWN_SCHEMAS: &[&str] = &[
    "500000000001.231", // measured on Plex 1.43.5
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
        Ok((KNOWN_SCHEMAS.contains(&v.as_str()), v))
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
    fn items_by_section() {
        let (_dir, db) = fixture("500000000001.231");
        assert_eq!(db.items_by_section(1).unwrap(), [42]);
        assert!(db.items_by_section(99).unwrap().is_empty());
    }
}

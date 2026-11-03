use rusqlite::params;

use super::Db;

/// marker_type values in metadata_item_setting_markers.
pub const MARKER_INTRO: i64 = 1;
pub const MARKER_CREDITS: i64 = 5;

/// The account that owns synthetic settings rows; 1 is the server owner
/// on standard installs.
const OWNER_ACCOUNT: i64 = 1;

impl Db {
    /// The metadata item that owns a given part file path, if any.
    pub fn item_id_by_part_file(&self, file: &str) -> rusqlite::Result<Option<i64>> {
        let id = self.conn.query_row(
            "SELECT mi.metadata_item_id
               FROM media_items mi JOIN media_parts p ON p.media_item_id = mi.id
              WHERE p.file = ?",
            [file],
            |r| r.get(0),
        );
        match id {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Writes (replaces) one marker on an item. A settings row is created
    /// on demand: Plex links markers through metadata_item_settings keyed
    /// by the item's guid, and rows appear lazily.
    pub fn write_marker(
        &mut self,
        item_id: i64,
        marker_type: i64,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<(), super::WriteError> {
        let (known, version) = self.schema_known()?;
        if !known {
            return Err(super::WriteError::UnknownSchema(version));
        }

        let tx = self.conn.transaction()?;
        let guid: String = tx.query_row(
            "SELECT guid FROM metadata_items WHERE id = ?",
            [item_id],
            |r| r.get(0),
        )?;
        let setting_id: Option<i64> = match tx.query_row(
            "SELECT id FROM metadata_item_settings WHERE guid = ? AND account_id = ?",
            params![guid, OWNER_ACCOUNT],
            |r| r.get(0),
        ) {
            Ok(id) => Some(id),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(e.into()),
        };
        let setting_id = match setting_id {
            Some(id) => id,
            None => {
                tx.execute(
                    "INSERT INTO metadata_item_settings (account_id, guid) VALUES (?, ?)",
                    params![OWNER_ACCOUNT, guid],
                )?;
                tx.last_insert_rowid()
            }
        };
        tx.execute(
            "DELETE FROM metadata_item_setting_markers
              WHERE metadata_item_setting_id = ? AND marker_type = ?",
            params![setting_id, marker_type],
        )?;
        tx.execute(
            "INSERT INTO metadata_item_setting_markers
                 (marker_type, metadata_item_setting_id,
                  start_time_offset, end_time_offset)
             VALUES (?, ?, ?, ?)",
            params![marker_type, setting_id, start_ms, end_ms],
        )?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture with the marker-side tables; item 42 with guid g42.
    fn db(dir: &std::path::Path) -> Db {
        let db = Db::open(dir.join("lib.db")).unwrap();
        db.conn
            .execute_batch(
                r#"CREATE TABLE schema_migrations (version varchar(255) NOT NULL);
                INSERT INTO schema_migrations VALUES ('500000000001.231');
                CREATE TABLE metadata_items (id INTEGER PRIMARY KEY, guid TEXT);
                INSERT INTO metadata_items VALUES (42, 'local://42');
                CREATE TABLE metadata_item_settings (
                  id INTEGER PRIMARY KEY, account_id INTEGER, guid TEXT,
                  rating INTEGER, view_offset INTEGER, view_count INTEGER,
                  last_viewed_at INTEGER, created_at INTEGER);
                CREATE TABLE metadata_item_setting_markers (
                  id INTEGER PRIMARY KEY AUTOINCREMENT,
                  marker_type INTEGER, metadata_item_setting_id INTEGER,
                  start_time_offset INTEGER, end_time_offset INTEGER,
                  title TEXT, created_at INTEGER, updated_at INTEGER, extra_data TEXT);"#,
            )
            .unwrap();
        db
    }

    fn count(db: &Db, sql: &str) -> i64 {
        db.conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn creates_settings_row_and_replaces_marker() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = db(dir.path());

        db.write_marker(42, MARKER_CREDITS, 30_000, 58_000).unwrap();
        assert_eq!(
            count(
                &db,
                "SELECT count(*) FROM metadata_item_settings WHERE guid='local://42'"
            ),
            1
        );
        assert_eq!(
            count(&db, "SELECT count(*) FROM metadata_item_setting_markers"),
            1
        );

        // a second write replaces instead of duplicating
        db.write_marker(42, MARKER_CREDITS, 32_000, 57_000).unwrap();
        assert_eq!(
            count(&db, "SELECT count(*) FROM metadata_item_setting_markers"),
            1
        );
        let (s, e): (i64, i64) = db
            .conn
            .query_row(
                "SELECT start_time_offset, end_time_offset
                   FROM metadata_item_setting_markers",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((s, e), (32_000, 57_000));

        // an intro marker coexists with the credits one
        db.write_marker(42, MARKER_INTRO, 1_000, 20_000).unwrap();
        assert_eq!(
            count(&db, "SELECT count(*) FROM metadata_item_setting_markers"),
            2
        );
    }
}

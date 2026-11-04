//! Writes Chinese genre, mood and style labels for one metadata item.
//!
//! Tag rows are shared, so a translated label is retagged rather than
//! renamed in place: another item may still be using the English row. Each
//! kind that changes is added to `user_fields`, which is how Plex records a
//! lock. Unlocking removes those ids and leaves the labels; a later metadata
//! refresh is what puts the source values back.

use rusqlite::params;

use super::Db;
use crate::localize::{self, LOCK_GENRE, LOCK_MOOD, LOCK_STYLE, LOCK_TITLE_SORT};

/// Plex tag types. Genre is what a film or show carries. Mood and style are
/// what music carries. The same lock ids apply to either.
const TAG_GENRE: i64 = 1;
const TAG_MOOD: i64 = 300;
const TAG_STYLE: i64 = 301;

/// One tag row hanging off an item.
struct TagLink {
    tagging_id: i64,
    tag_type: i64,
    tag: String,
}

impl Db {
    /// Writes the sort title from the stored display title, and locks it.
    /// A locked sort title is left as stored. A title with nothing to sort
    /// is left as stored. Returns whether the sort title changed.
    pub fn localize_sort(&mut self, item_id: i64) -> Result<bool, super::WriteError> {
        self.write_known(|tx| {
            let (title, title_sort, user_fields) = item_title(tx, item_id)?;
            if field_locked(&user_fields, LOCK_TITLE_SORT) {
                return Ok(false);
            }
            let Some(sort) = localize::sort_title(&title) else {
                return Ok(false);
            };
            if title_sort.as_deref() == Some(sort.as_str()) {
                return Ok(false);
            }
            let locks = localize::lock_field(&user_fields, LOCK_TITLE_SORT);
            tx.execute(
                "UPDATE metadata_items SET title_sort=?, user_fields=? WHERE id=?",
                params![sort, locks, item_id],
            )?;
            Ok(true)
        })
    }

    /// Drops the sort-title lock on an item. The sort title stays. Returns
    /// whether a lock was removed.
    pub fn unlock_sort(&mut self, item_id: i64) -> Result<bool, super::WriteError> {
        self.write_known(|tx| {
            let user_fields = item_fields(tx, item_id)?;
            let locks = localize::unlock_field(&user_fields, LOCK_TITLE_SORT);
            if locks == user_fields {
                return Ok(false);
            }
            let stored = if locks.is_empty() { None } else { Some(locks) };
            tx.execute(
                "UPDATE metadata_items SET user_fields=? WHERE id=?",
                params![stored, item_id],
            )?;
            Ok(true)
        })
    }

    /// Rewrites genre, mood and style labels that the table knows, and locks
    /// each kind that changed. A locked kind is left as stored.
    pub fn localize_tags(&mut self, item_id: i64) -> Result<usize, super::WriteError> {
        self.write_known(|tx| {
            let user_fields = item_fields(tx, item_id)?;
            let links = tag_links(tx, item_id)?;
            let mut changed = 0;
            let mut locks = user_fields;
            for link in links {
                let lock_id = lock_of(link.tag_type);
                if field_locked(&locks, lock_id) {
                    continue;
                }
                let Some(chinese) = localize::translate_tag(&link.tag) else {
                    continue;
                };
                if chinese == link.tag {
                    continue;
                }
                let new_id = find_or_insert_tag(tx, link.tag_type, chinese)?;
                tx.execute(
                    "UPDATE taggings SET tag_id=? WHERE id=?",
                    params![new_id, link.tagging_id],
                )?;
                locks = localize::lock_field(&locks, lock_id);
                changed += 1;
            }
            if changed > 0 {
                let denorm = denormalized_tags(tx, item_id, TAG_GENRE)?;
                tx.execute(
                    "UPDATE metadata_items SET user_fields=?, tags_genre=? WHERE id=?",
                    params![locks, denorm, item_id],
                )?;
            }
            Ok(changed)
        })
    }

    /// Drops the genre, mood and style locks on an item. The labels stay.
    /// Returns whether a lock was removed.
    pub fn unlock_tags(&mut self, item_id: i64) -> Result<bool, super::WriteError> {
        self.write_known(|tx| {
            let user_fields = item_fields(tx, item_id)?;
            let mut locks = user_fields.clone();
            for id in localize::TAG_LOCKS {
                locks = localize::unlock_field(&locks, id);
            }
            if locks == user_fields {
                return Ok(false);
            }
            let stored = if locks.is_empty() { None } else { Some(locks) };
            tx.execute(
                "UPDATE metadata_items SET user_fields=? WHERE id=?",
                params![stored, item_id],
            )?;
            Ok(true)
        })
    }

    fn write_known<T>(
        &mut self,
        body: impl FnOnce(&rusqlite::Transaction) -> Result<T, super::WriteError>,
    ) -> Result<T, super::WriteError> {
        let (known, version) = self.schema_known()?;
        if !known {
            return Err(super::WriteError::UnknownSchema(version));
        }
        let tx = self.conn.transaction()?;
        let out = body(&tx)?;
        tx.commit()?;
        Ok(out)
    }
}

fn lock_of(tag_type: i64) -> i64 {
    match tag_type {
        TAG_MOOD => LOCK_MOOD,
        TAG_STYLE => LOCK_STYLE,
        _ => LOCK_GENRE,
    }
}

fn field_locked(user_fields: &str, id: i64) -> bool {
    user_fields.split('|').any(|part| {
        let Some(list) = part.strip_prefix("lockedFields=") else {
            return false;
        };
        list.split('|').any(|s| s.parse::<i64>().ok() == Some(id))
    })
}

fn item_title(
    tx: &rusqlite::Transaction,
    item_id: i64,
) -> Result<(String, Option<String>, String), rusqlite::Error> {
    tx.query_row(
        "SELECT IFNULL(title, ''), title_sort, IFNULL(user_fields, '')
         FROM metadata_items WHERE id=?",
        [item_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
}

fn item_fields(tx: &rusqlite::Transaction, item_id: i64) -> Result<String, rusqlite::Error> {
    tx.query_row(
        "SELECT IFNULL(user_fields, '') FROM metadata_items WHERE id=?",
        [item_id],
        |row| row.get(0),
    )
}

fn tag_links(tx: &rusqlite::Transaction, item_id: i64) -> Result<Vec<TagLink>, rusqlite::Error> {
    let mut stmt = tx.prepare(
        "SELECT tg.id, t.tag_type, t.tag
         FROM taggings tg JOIN tags t ON t.id = tg.tag_id
         WHERE tg.metadata_item_id=? AND t.tag_type IN (1, 300, 301)
         ORDER BY tg.\"index\"",
    )?;
    let rows = stmt.query_map([item_id], |row| {
        Ok(TagLink {
            tagging_id: row.get(0)?,
            tag_type: row.get(1)?,
            tag: row.get(2)?,
        })
    })?;
    rows.collect()
}

fn find_or_insert_tag(
    tx: &rusqlite::Transaction,
    tag_type: i64,
    tag: &str,
) -> Result<i64, rusqlite::Error> {
    let existing = tx.query_row(
        "SELECT id FROM tags WHERE tag_type=? AND tag=? AND metadata_item_id IS NULL LIMIT 1",
        params![tag_type, tag],
        |row| row.get(0),
    );
    match existing {
        Ok(id) => Ok(id),
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            tx.execute(
                "INSERT INTO tags (tag, tag_type, created_at, updated_at)
                 VALUES (?, ?, strftime('%s','now'), strftime('%s','now'))",
                params![tag, tag_type],
            )?;
            Ok(tx.last_insert_rowid())
        }
        Err(e) => Err(e),
    }
}

fn denormalized_tags(
    tx: &rusqlite::Transaction,
    item_id: i64,
    tag_type: i64,
) -> Result<String, rusqlite::Error> {
    let mut stmt = tx.prepare(
        "SELECT t.tag FROM taggings tg JOIN tags t ON t.id = tg.tag_id
         WHERE tg.metadata_item_id=? AND t.tag_type=?
         ORDER BY tg.\"index\"",
    )?;
    let rows = stmt.query_map(params![item_id, tag_type], |row| row.get::<_, String>(0))?;
    let mut out = String::new();
    for row in rows {
        if !out.is_empty() {
            out.push('|');
        }
        out.push_str(&row?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use crate::plexdb::Db;

    const SCHEMA: &str = r#"
        CREATE TABLE schema_migrations (version varchar(255) NOT NULL);
        INSERT INTO schema_migrations VALUES ('500000000001.231');
        CREATE TABLE metadata_items (
          id INTEGER PRIMARY KEY, title TEXT, title_sort TEXT, user_fields TEXT, tags_genre TEXT);
        CREATE TABLE tags (
          id INTEGER PRIMARY KEY AUTOINCREMENT, metadata_item_id INTEGER,
          tag TEXT, tag_type INTEGER, created_at INTEGER, updated_at INTEGER);
        CREATE TABLE taggings (
          id INTEGER PRIMARY KEY AUTOINCREMENT, metadata_item_id INTEGER,
          tag_id INTEGER, "index" INTEGER);
    "#;

    fn db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("lib.db")).unwrap();
        db.conn.execute_batch(SCHEMA).unwrap();
        (dir, db)
    }

    #[test]
    fn tags_move_to_the_chinese_row_and_lock() {
        let (_dir, mut db) = db();
        db.conn
            .execute_batch(
                "INSERT INTO metadata_items (id, title) VALUES (1, 'show');
                 INSERT INTO tags (id, tag, tag_type) VALUES (6, 'Action', 1);
                 INSERT INTO taggings (metadata_item_id, tag_id, \"index\") VALUES (1, 6, 0);",
            )
            .unwrap();
        assert_eq!(db.localize_tags(1).unwrap(), 1);
        let (tag, fields, genre): (String, String, String) = db
            .conn
            .query_row(
                "SELECT t.tag, m.user_fields, m.tags_genre
                 FROM metadata_items m
                 JOIN taggings tg ON tg.metadata_item_id = m.id
                 JOIN tags t ON t.id = tg.tag_id
                 WHERE m.id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(tag, "动作");
        assert_eq!(fields, "lockedFields=1");
        assert_eq!(genre, "动作");
        // the English row stays, for any other item still using it
        let english: String = db
            .conn
            .query_row("SELECT tag FROM tags WHERE id=6", [], |row| row.get(0))
            .unwrap();
        assert_eq!(english, "Action");
        // locked, so a second pass does not rewrite
        assert_eq!(db.localize_tags(1).unwrap(), 0);
    }

    #[test]
    fn unlock_drops_the_lock_and_keeps_the_label() {
        let (_dir, mut db) = db();
        db.conn
            .execute(
                "INSERT INTO metadata_items (id, user_fields, tags_genre)
                 VALUES (1, 'lockedFields=1|2', '动作')",
                [],
            )
            .unwrap();
        assert!(db.unlock_tags(1).unwrap());
        let (fields, genre): (String, String) = db
            .conn
            .query_row(
                "SELECT user_fields, tags_genre FROM metadata_items WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(fields, "lockedFields=2");
        assert_eq!(genre, "动作");
        assert!(!db.unlock_tags(1).unwrap());
    }

    #[test]
    fn sort_title_is_written_and_locked() {
        let (_dir, mut db) = db();
        db.conn
            .execute(
                "INSERT INTO metadata_items (id, title, title_sort) VALUES (1, '重庆森林', '重庆森林')",
                [],
            )
            .unwrap();
        assert!(db.localize_sort(1).unwrap());
        let (title, sort, fields): (String, String, String) = db
            .conn
            .query_row(
                "SELECT title, title_sort, user_fields FROM metadata_items WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(title, "重庆森林");
        assert_eq!(sort, "CQSL");
        assert_eq!(fields, "lockedFields=2");
        assert!(!db.localize_sort(1).unwrap());
    }

    #[test]
    fn a_locked_sort_title_is_left() {
        let (_dir, mut db) = db();
        db.conn
            .execute(
                "INSERT INTO metadata_items (id, title, title_sort, user_fields)
                 VALUES (1, '重庆森林', 'KEEP', 'lockedFields=2')",
                [],
            )
            .unwrap();
        assert!(!db.localize_sort(1).unwrap());
        let sort: String = db
            .conn
            .query_row(
                "SELECT title_sort FROM metadata_items WHERE id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sort, "KEEP");
    }

    #[test]
    fn unlock_drops_the_sort_lock_and_keeps_the_sort_title() {
        let (_dir, mut db) = db();
        db.conn
            .execute(
                "INSERT INTO metadata_items (id, title, title_sort, user_fields)
                 VALUES (1, '重庆森林', 'CQSL', 'lockedFields=1|2')",
                [],
            )
            .unwrap();
        assert!(db.unlock_sort(1).unwrap());
        let (sort, fields): (String, String) = db
            .conn
            .query_row(
                "SELECT title_sort, user_fields FROM metadata_items WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(sort, "CQSL");
        assert_eq!(fields, "lockedFields=1");
        assert!(!db.unlock_sort(1).unwrap());
    }
}

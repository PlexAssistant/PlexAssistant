//! Writes Chinese genre, mood and style labels, and Chinese cast names, for
//! one metadata item.
//!
//! Tag rows are shared, so a translated label is retagged rather than
//! renamed in place: another item may still be using the English row. Each
//! kind that changes is added to `user_fields`, which is how Plex records a
//! lock. Unlocking removes those ids and leaves the labels; a later metadata
//! refresh is what puts the source values back.

use rusqlite::params;

use super::Db;
use crate::localize::{
    self, Credit, CreditTarget, Credits, LOCK_ACTOR, LOCK_DIRECTOR, LOCK_GENRE, LOCK_MOOD,
    LOCK_STYLE, LOCK_TITLE_SORT, LOCK_WRITER,
};

/// Plex tag types. Genre is what a film or show carries. Mood and style are
/// what music carries. The same lock ids apply to either.
const TAG_GENRE: i64 = 1;
const TAG_MOOD: i64 = 300;
const TAG_STYLE: i64 = 301;

/// Plex tag types for people. The role an actor plays stays in
/// `taggings.text`; only the person's name moves.
const TAG_DIRECTOR: i64 = 4;
const TAG_WRITER: i64 = 5;
const TAG_ACTOR: i64 = 6;

/// External ids. `tmdb://42` on a show, a season or an episode.
const TAG_GUID: i64 = 314;

/// metadata_items.metadata_type. A film is 1, a show 2, a season 3, an
/// episode 4.
const TYPE_MOVIE: i64 = 1;
const TYPE_SHOW: i64 = 2;
const TYPE_SEASON: i64 = 3;
const TYPE_EPISODE: i64 = 4;

/// One tag row hanging off an item.
struct TagLink {
    tagging_id: i64,
    tag_type: i64,
    tag: String,
    /// The photo on the person's row. Genre, mood and style rows carry none.
    thumb: Option<String>,
}

impl Db {
    /// Writes the sort title from the stored display title, and locks it.
    /// A locked sort title is left as stored. A title with nothing to sort
    /// is left as stored. Returns whether the sort title changed.
    pub fn localize_sort(&mut self, item_id: i64) -> Result<bool, super::WriteError> {
        self.write_known(|tx| {
            let (title, title_sort, user_fields) = item_title(tx, item_id)?;
            if localize::field_is_locked(&user_fields, LOCK_TITLE_SORT) {
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
    /// each kind that changed. A kind that arrived locked is left as stored;
    /// the lock this pass writes does not gate the rest of the pass, so an
    /// item carrying several labels of one kind has them all move together.
    pub fn localize_tags(&mut self, item_id: i64) -> Result<usize, super::WriteError> {
        self.write_known(|tx| {
            let user_fields = item_fields(tx, item_id)?;
            let links = tag_links(tx, item_id)?;
            let mut changed = 0;
            let mut locks = user_fields.clone();
            for link in links {
                let lock_id = lock_of(link.tag_type);
                if localize::field_is_locked(&user_fields, lock_id) {
                    continue;
                }
                let Some(chinese) = localize::translate_tag(&link.tag) else {
                    continue;
                };
                if chinese == link.tag {
                    continue;
                }
                let new_id = find_or_insert_tag(tx, link.tag_type, chinese, None)?;
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

    /// The credits list this item's cast should be read from, or nothing
    /// when the item has no TMDB id. A film uses its own id. An episode uses
    /// its show's id with the season and episode numbers. A show uses its
    /// own id only when it carries the cast itself.
    pub fn credit_target(&self, item_id: i64) -> Result<Option<CreditTarget>, rusqlite::Error> {
        let Some(item) = item_row(&self.conn, item_id)? else {
            return Ok(None);
        };
        let target = match item.metadata_type {
            TYPE_MOVIE => tmdb_id(&self.conn, item_id)?.map(CreditTarget::Movie),
            TYPE_EPISODE => episode_target(&self.conn, &item)?,
            TYPE_SHOW => show_target(&self.conn, item_id)?,
            _ => None,
        };
        Ok(target)
    }

    /// Rewrites director, writer and actor names that `credits` names, and
    /// locks each kind that changed. A person is matched on the original
    /// name. A locked kind is left as stored. A name that did not change is
    /// not locked.
    pub fn localize_people(
        &mut self,
        item_id: i64,
        credits: &Credits,
    ) -> Result<usize, super::WriteError> {
        self.write_known(|tx| {
            let user_fields = item_fields(tx, item_id)?;
            let mut changed = 0;
            let mut locks = user_fields;
            for (tag_type, lock_id, list) in [
                (TAG_DIRECTOR, LOCK_DIRECTOR, &credits.directors),
                (TAG_WRITER, LOCK_WRITER, &credits.writers),
                (TAG_ACTOR, LOCK_ACTOR, &credits.cast),
            ] {
                if localize::field_is_locked(&locks, lock_id) {
                    continue;
                }
                if retag_people(tx, item_id, tag_type, list)? == 0 {
                    continue;
                }
                locks = localize::lock_field(&locks, lock_id);
                let names = denormalized_tags(tx, item_id, tag_type)?;
                let sql = match tag_type {
                    TAG_DIRECTOR => {
                        "UPDATE metadata_items SET user_fields=?, tags_director=? WHERE id=?"
                    }
                    TAG_WRITER => {
                        "UPDATE metadata_items SET user_fields=?, tags_writer=? WHERE id=?"
                    }
                    _ => "UPDATE metadata_items SET user_fields=?, tags_star=? WHERE id=?",
                };
                tx.execute(sql, params![locks, names, item_id])?;
                changed += 1;
            }
            Ok(changed)
        })
    }

    /// Drops the director, writer and actor locks on an item. The names
    /// stay. Returns whether a lock was removed.
    pub fn unlock_people(&mut self, item_id: i64) -> Result<bool, super::WriteError> {
        self.write_known(|tx| {
            let user_fields = item_fields(tx, item_id)?;
            let mut locks = user_fields.clone();
            for id in localize::PEOPLE_LOCKS {
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

/// One metadata row, enough to find the credits list it belongs to.
struct ItemRow {
    metadata_type: i64,
    parent_id: Option<i64>,
    index: Option<i64>,
}

fn item_row(conn: &rusqlite::Connection, item_id: i64) -> Result<Option<ItemRow>, rusqlite::Error> {
    let row = conn.query_row(
        "SELECT metadata_type, parent_id, \"index\" FROM metadata_items WHERE id=?",
        [item_id],
        |row| {
            Ok(ItemRow {
                metadata_type: row.get(0)?,
                parent_id: row.get(1)?,
                index: row.get(2)?,
            })
        },
    );
    match row {
        Ok(row) => Ok(Some(row)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// An episode's credits: the show's TMDB id, the season number and the
/// episode number. A season or a show without that id has none.
fn episode_target(
    conn: &rusqlite::Connection,
    episode: &ItemRow,
) -> Result<Option<CreditTarget>, rusqlite::Error> {
    let (Some(season_id), Some(episode_index)) = (episode.parent_id, episode.index) else {
        return Ok(None);
    };
    if episode_index <= 0 {
        return Ok(None);
    }
    let Some(season) = item_row(conn, season_id)? else {
        return Ok(None);
    };
    if season.metadata_type != TYPE_SEASON {
        return Ok(None);
    }
    let (Some(show_id), Some(season_index)) = (season.parent_id, season.index) else {
        return Ok(None);
    };
    if season_index <= 0 {
        return Ok(None);
    }
    let Some(show_tmdb) = tmdb_id(conn, show_id)? else {
        return Ok(None);
    };
    Ok(Some(CreditTarget::Episode {
        id: show_tmdb,
        season: season_index as u32,
        episode: episode_index as u32,
    }))
}

/// A show that carries its own cast uses the show credits. A show whose
/// cast lives on the episodes has nothing to rewrite at this level.
fn show_target(
    conn: &rusqlite::Connection,
    show_id: i64,
) -> Result<Option<CreditTarget>, rusqlite::Error> {
    let has_people: i64 = conn.query_row(
        "SELECT COUNT(*) FROM taggings tg JOIN tags t ON t.id = tg.tag_id
         WHERE tg.metadata_item_id=? AND t.tag_type IN (4, 5, 6)",
        [show_id],
        |row| row.get(0),
    )?;
    if has_people == 0 {
        return Ok(None);
    }
    Ok(tmdb_id(conn, show_id)?.map(CreditTarget::Show))
}

/// The number in a `tmdb://` guid tag, when the item has one.
fn tmdb_id(conn: &rusqlite::Connection, item_id: i64) -> Result<Option<u64>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT t.tag FROM taggings tg JOIN tags t ON t.id = tg.tag_id
         WHERE tg.metadata_item_id=? AND t.tag_type=? AND t.tag LIKE 'tmdb://%'",
    )?;
    let mut rows = stmt.query(params![item_id, TAG_GUID])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let tag: String = row.get(0)?;
    Ok(tag.trim_start_matches("tmdb://").parse().ok())
}

/// Points each person whose stored name is still the original at the Chinese
/// name. The role in `taggings.text` stays, and the photo on the row moves
/// with the name. Returns how many names moved.
fn retag_people(
    tx: &rusqlite::Transaction,
    item_id: i64,
    tag_type: i64,
    credits: &[Credit],
) -> Result<usize, rusqlite::Error> {
    let links = people_links(tx, item_id, tag_type)?;
    let mut changed = 0;
    for link in links {
        let Some(credit) = credits
            .iter()
            .find(|c| c.original == link.tag || c.name == link.tag)
        else {
            continue;
        };
        if credit.name == link.tag {
            continue;
        }
        let new_id = find_or_insert_tag(tx, tag_type, &credit.name, link.thumb.as_deref())?;
        tx.execute(
            "UPDATE taggings SET tag_id=? WHERE id=?",
            params![new_id, link.tagging_id],
        )?;
        changed += 1;
    }
    Ok(changed)
}

fn people_links(
    tx: &rusqlite::Transaction,
    item_id: i64,
    tag_type: i64,
) -> Result<Vec<TagLink>, rusqlite::Error> {
    let mut stmt = tx.prepare(
        "SELECT tg.id, t.tag_type, t.tag, t.user_thumb_url
         FROM taggings tg JOIN tags t ON t.id = tg.tag_id
         WHERE tg.metadata_item_id=? AND t.tag_type=?
         ORDER BY tg.\"index\"",
    )?;
    let rows = stmt.query_map(params![item_id, tag_type], |row| {
        Ok(TagLink {
            tagging_id: row.get(0)?,
            tag_type: row.get(1)?,
            tag: row.get(2)?,
            thumb: row.get(3)?,
        })
    })?;
    rows.collect()
}

fn lock_of(tag_type: i64) -> i64 {
    match tag_type {
        TAG_MOOD => LOCK_MOOD,
        TAG_STYLE => LOCK_STYLE,
        _ => LOCK_GENRE,
    }
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
        "SELECT tg.id, t.tag_type, t.tag, t.user_thumb_url
         FROM taggings tg JOIN tags t ON t.id = tg.tag_id
         WHERE tg.metadata_item_id=? AND t.tag_type IN (1, 300, 301)
         ORDER BY tg.\"index\"",
    )?;
    let rows = stmt.query_map([item_id], |row| {
        Ok(TagLink {
            tagging_id: row.get(0)?,
            tag_type: row.get(1)?,
            tag: row.get(2)?,
            thumb: row.get(3)?,
        })
    })?;
    rows.collect()
}

/// The id of the tag row for `tag`, creating it when the library has none.
/// A people row is created carrying `thumb`, and a row that already exists
/// without one is filled in, so the photo stays with the name.
fn find_or_insert_tag(
    tx: &rusqlite::Transaction,
    tag_type: i64,
    tag: &str,
    thumb: Option<&str>,
) -> Result<i64, rusqlite::Error> {
    let existing = tx.query_row(
        "SELECT id, user_thumb_url FROM tags
         WHERE tag_type=? AND tag=? AND metadata_item_id IS NULL LIMIT 1",
        params![tag_type, tag],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
    );
    match existing {
        Ok((id, have)) => {
            let missing = matches!(have.as_deref(), None | Some(""));
            if let (Some(new), true) = (thumb, missing) {
                tx.execute(
                    "UPDATE tags SET user_thumb_url=? WHERE id=?",
                    params![new, id],
                )?;
            }
            Ok(id)
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            tx.execute(
                "INSERT INTO tags (tag, tag_type, user_thumb_url, created_at, updated_at)
                 VALUES (?, ?, ?, strftime('%s','now'), strftime('%s','now'))",
                params![tag, tag_type, thumb],
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
          id INTEGER PRIMARY KEY, metadata_type INTEGER, parent_id INTEGER,
          "index" INTEGER, title TEXT, title_sort TEXT, user_fields TEXT,
          tags_genre TEXT, tags_director TEXT, tags_writer TEXT, tags_star TEXT);
        CREATE TABLE tags (
          id INTEGER PRIMARY KEY AUTOINCREMENT, metadata_item_id INTEGER,
          tag TEXT, tag_type INTEGER, user_thumb_url TEXT,
          created_at INTEGER, updated_at INTEGER);
        CREATE TABLE taggings (
          id INTEGER PRIMARY KEY AUTOINCREMENT, metadata_item_id INTEGER,
          tag_id INTEGER, "index" INTEGER, text TEXT);
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
    fn every_label_of_a_kind_moves_in_one_pass() {
        let (_dir, mut db) = db();
        db.conn
            .execute_batch(
                "INSERT INTO metadata_items (id, title) VALUES (1, 'show');
                 INSERT INTO tags (id, tag, tag_type) VALUES
                   (6, 'Action', 1), (7, 'Adventure', 1), (8, '剧情', 1);
                 INSERT INTO taggings (metadata_item_id, tag_id, \"index\") VALUES
                   (1, 6, 0), (1, 7, 1), (1, 8, 2);",
            )
            .unwrap();
        // the lock the pass writes after Action must not skip Adventure
        assert_eq!(db.localize_tags(1).unwrap(), 2);
        let (fields, genre): (String, String) = db
            .conn
            .query_row(
                "SELECT user_fields, tags_genre FROM metadata_items WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(fields, "lockedFields=1");
        assert_eq!(genre, "动作|冒险|剧情");
    }

    #[test]
    fn a_tag_lock_behind_another_id_is_respected() {
        let (_dir, mut db) = db();
        db.conn
            .execute_batch(
                "INSERT INTO metadata_items (id, user_fields) VALUES (1, 'lockedFields=2|1');
                 INSERT INTO tags (id, tag, tag_type) VALUES (6, 'Action', 1);
                 INSERT INTO taggings (metadata_item_id, tag_id, \"index\") VALUES (1, 6, 0);",
            )
            .unwrap();
        assert_eq!(db.localize_tags(1).unwrap(), 0);
        let tag: String = db
            .conn
            .query_row("SELECT tag FROM tags WHERE id=6", [], |row| row.get(0))
            .unwrap();
        assert_eq!(tag, "Action");
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
    fn a_sort_lock_behind_another_id_is_respected() {
        let (_dir, mut db) = db();
        // genre (1) was locked first, so the sort lock (2) is not the piece
        // that carries the lockedFields= prefix
        db.conn
            .execute(
                "INSERT INTO metadata_items (id, title, title_sort, user_fields)
                 VALUES (1, '重庆森林', 'MY OWN', 'lockedFields=1|2')",
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
        assert_eq!(sort, "MY OWN");
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

    fn credit(name: &str, original: &str) -> crate::localize::Credit {
        crate::localize::Credit {
            name: name.into(),
            original: original.into(),
        }
    }

    #[test]
    fn an_episode_points_at_its_show_season_and_number() {
        let (_dir, db) = db();
        db.conn
            .execute_batch(
                "INSERT INTO metadata_items (id, metadata_type, title) VALUES (10, 2, '示例剧');
                 INSERT INTO metadata_items (id, metadata_type, parent_id, \"index\")
                   VALUES (11, 3, 10, 2);
                 INSERT INTO metadata_items (id, metadata_type, parent_id, \"index\", title)
                   VALUES (12, 4, 11, 4, '示例集');
                 INSERT INTO tags (id, tag, tag_type) VALUES (1, 'tmdb://42', 314);
                 INSERT INTO taggings (metadata_item_id, tag_id) VALUES (10, 1);",
            )
            .unwrap();
        assert_eq!(
            db.credit_target(12).unwrap(),
            Some(crate::localize::CreditTarget::Episode {
                id: 42,
                season: 2,
                episode: 4,
            })
        );
        assert_eq!(db.credit_target(11).unwrap(), None);
    }

    #[test]
    fn a_film_uses_its_own_tmdb_id() {
        let (_dir, db) = db();
        db.conn
            .execute_batch(
                "INSERT INTO metadata_items (id, metadata_type, title) VALUES (1, 1, '示例电影');
                 INSERT INTO tags (id, tag, tag_type) VALUES (1, 'tmdb://7', 314);
                 INSERT INTO taggings (metadata_item_id, tag_id) VALUES (1, 1);",
            )
            .unwrap();
        assert_eq!(
            db.credit_target(1).unwrap(),
            Some(crate::localize::CreditTarget::Movie(7))
        );
    }

    #[test]
    fn a_show_with_its_own_cast_uses_the_show() {
        let (_dir, db) = db();
        db.conn
            .execute_batch(
                "INSERT INTO metadata_items (id, metadata_type, title) VALUES (10, 2, '示例剧');
                 INSERT INTO tags (id, tag, tag_type) VALUES (1, 'tmdb://42', 314), (2, 'Ada Actor', 6);
                 INSERT INTO taggings (metadata_item_id, tag_id) VALUES (10, 1), (10, 2);",
            )
            .unwrap();
        assert_eq!(
            db.credit_target(10).unwrap(),
            Some(crate::localize::CreditTarget::Show(42))
        );
    }

    #[test]
    fn people_move_to_the_chinese_name_and_lock() {
        let (_dir, mut db) = db();
        db.conn
            .execute_batch(
                "INSERT INTO metadata_items (id, metadata_type, title) VALUES (1, 1, '示例电影');
                 INSERT INTO tags (id, tag, tag_type) VALUES
                   (4, 'Ada Director', 4), (5, 'Ada Writer', 5), (6, 'Ada Actor', 6);
                 INSERT INTO taggings (id, metadata_item_id, tag_id, \"index\", text) VALUES
                   (1, 1, 4, 0, '导演'), (2, 1, 5, 0, '编剧'), (3, 1, 6, 0, 'Lead');",
            )
            .unwrap();
        let credits = crate::localize::Credits {
            directors: vec![credit("甲导演", "Ada Director")],
            writers: vec![credit("甲编剧", "Ada Writer")],
            cast: vec![credit("甲演员", "Ada Actor")],
        };
        assert_eq!(db.localize_people(1, &credits).unwrap(), 3);
        let (director, writer, star, fields): (String, String, String, String) = db
            .conn
            .query_row(
                "SELECT tags_director, tags_writer, tags_star, user_fields
                 FROM metadata_items WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(director, "甲导演");
        assert_eq!(writer, "甲编剧");
        assert_eq!(star, "甲演员");
        assert_eq!(fields, "lockedFields=17|18|19");
        let role: String = db
            .conn
            .query_row("SELECT text FROM taggings WHERE id=3", [], |row| row.get(0))
            .unwrap();
        assert_eq!(role, "Lead");
        let english: String = db
            .conn
            .query_row("SELECT tag FROM tags WHERE id=4", [], |row| row.get(0))
            .unwrap();
        assert_eq!(english, "Ada Director");
        assert_eq!(db.localize_people(1, &credits).unwrap(), 0);
    }

    #[test]
    fn a_locked_cast_is_left() {
        let (_dir, mut db) = db();
        db.conn
            .execute_batch(
                "INSERT INTO metadata_items (id, user_fields, tags_star)
                   VALUES (1, 'lockedFields=19', 'Ada Actor');
                 INSERT INTO tags (id, tag, tag_type) VALUES (6, 'Ada Actor', 6);
                 INSERT INTO taggings (metadata_item_id, tag_id) VALUES (1, 6);",
            )
            .unwrap();
        let credits = crate::localize::Credits {
            cast: vec![credit("甲演员", "Ada Actor")],
            ..Default::default()
        };
        assert_eq!(db.localize_people(1, &credits).unwrap(), 0);
        let star: String = db
            .conn
            .query_row(
                "SELECT tags_star FROM metadata_items WHERE id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(star, "Ada Actor");
    }

    #[test]
    fn a_retagged_person_keeps_the_photo() {
        let (_dir, mut db) = db();
        db.conn
            .execute_batch(
                "INSERT INTO metadata_items (id, metadata_type, title) VALUES (1, 1, '示例电影');
                 INSERT INTO tags (id, tag, tag_type, user_thumb_url) VALUES
                   (4, 'Ada Director', 4, 'https://img.example/ada.jpg'),
                   (5, 'Ada Writer', 5, 'https://img.example/writer.jpg'),
                   (6, '甲导演', 4, NULL);
                 INSERT INTO taggings (metadata_item_id, tag_id, \"index\") VALUES
                   (1, 4, 0), (1, 5, 1);",
            )
            .unwrap();
        let credits = crate::localize::Credits {
            directors: vec![credit("甲导演", "Ada Director")],
            writers: vec![credit("甲编剧", "Ada Writer")],
            ..Default::default()
        };
        assert_eq!(db.localize_people(1, &credits).unwrap(), 2);
        // the row that already existed without a photo is filled in
        let director: Option<String> = db
            .conn
            .query_row("SELECT user_thumb_url FROM tags WHERE id=6", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(director.as_deref(), Some("https://img.example/ada.jpg"));
        // the row that was created carries the photo
        let writer: Option<String> = db
            .conn
            .query_row(
                "SELECT user_thumb_url FROM tags WHERE tag='甲编剧'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(writer.as_deref(), Some("https://img.example/writer.jpg"));
        // the English rows keep theirs
        let english: Option<String> = db
            .conn
            .query_row("SELECT user_thumb_url FROM tags WHERE id=4", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(english.as_deref(), Some("https://img.example/ada.jpg"));
    }

    #[test]
    fn a_people_lock_behind_another_id_is_respected() {
        let (_dir, mut db) = db();
        // director (17) was locked first, so the actor lock (19) is a bare id
        db.conn
            .execute_batch(
                "INSERT INTO metadata_items (id, user_fields) VALUES (1, 'lockedFields=17|19');
                 INSERT INTO tags (id, tag, tag_type) VALUES (6, 'Ada Actor', 6);
                 INSERT INTO taggings (metadata_item_id, tag_id) VALUES (1, 6);",
            )
            .unwrap();
        let credits = crate::localize::Credits {
            cast: vec![credit("甲演员", "Ada Actor")],
            ..Default::default()
        };
        assert_eq!(db.localize_people(1, &credits).unwrap(), 0);
        let tag: String = db
            .conn
            .query_row("SELECT tag FROM tags WHERE id=6", [], |row| row.get(0))
            .unwrap();
        assert_eq!(tag, "Ada Actor");
    }

    #[test]
    fn unlock_drops_the_people_locks_and_keeps_the_names() {
        let (_dir, mut db) = db();
        db.conn
            .execute(
                "INSERT INTO metadata_items (id, user_fields, tags_star)
                 VALUES (1, 'lockedFields=1|17|18|19', '甲演员')",
                [],
            )
            .unwrap();
        assert!(db.unlock_people(1).unwrap());
        let (fields, star): (String, String) = db
            .conn
            .query_row(
                "SELECT user_fields, tags_star FROM metadata_items WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(fields, "lockedFields=1");
        assert_eq!(star, "甲演员");
        assert!(!db.unlock_people(1).unwrap());
    }
}

//! Audio and subtitle choices Plex already stored, and the ones we keep.
//!
//! A rewrite deletes `media_streams` and inserts new rows, so the stream ids
//! in `media_part_settings` stop pointing at anything. The previous ids are
//! remembered first, then pointed at the streams that just replaced them.
//! A choice is also kept per account and per show, matched by language and
//! stream index, because the next episode has different stream ids.

use rusqlite::{OptionalExtension, params};

use super::{Db, WriteError};

#[cfg(test)]
mod tests {
    use super::*;

    fn db(dir: &std::path::Path) -> Db {
        let db = Db::open(dir.join("lib.db")).unwrap();
        db.conn
            .execute_batch(
                r#"CREATE TABLE schema_migrations (version varchar(255) NOT NULL);
                INSERT INTO schema_migrations VALUES ('500000000001.231');
                CREATE TABLE metadata_items (
                  id INTEGER PRIMARY KEY, parent_id INTEGER, metadata_type INTEGER, guid TEXT);
                INSERT INTO metadata_items VALUES (1, NULL, 2, 'show');
                INSERT INTO metadata_items VALUES (2, 1, 3, 'season');
                INSERT INTO metadata_items VALUES (42, 2, 4, 'ep');
                CREATE TABLE media_streams (
                  id INTEGER PRIMARY KEY, stream_type_id INTEGER, media_part_id INTEGER,
                  language TEXT, "index" INTEGER, codec TEXT);
                INSERT INTO media_streams VALUES (10, 2, 70, 'ja', 2, 'aac');
                INSERT INTO media_streams VALUES (11, 3, 70, 'en', 3, 'srt');
                INSERT INTO media_streams VALUES (20, 2, 71, 'ja', 2, 'aac');
                INSERT INTO media_streams VALUES (21, 2, 71, 'en', 1, 'aac');
                INSERT INTO media_streams VALUES (30, 2, 72, 'de', 1, 'aac');
                CREATE TABLE media_part_settings (
                  id INTEGER PRIMARY KEY AUTOINCREMENT,
                  account_id INTEGER, media_part_id INTEGER,
                  selected_audio_stream_id INTEGER, selected_subtitle_stream_id INTEGER,
                  settings TEXT, created_at INTEGER, updated_at INTEGER, changed_at INTEGER);"#,
            )
            .unwrap();
        db
    }

    #[test]
    fn an_episode_names_its_show() {
        let dir = tempfile::tempdir().unwrap();
        let db = db(dir.path());
        assert_eq!(db.show_of_item(42).unwrap(), Some(1));
        assert_eq!(db.show_of_item(1).unwrap(), None);
    }

    #[test]
    fn a_choice_matches_language_and_index() {
        let dir = tempfile::tempdir().unwrap();
        let db = db(dir.path());
        let ja = TrackChoice {
            type_id: 2,
            language: "ja".into(),
            index: 2,
            codec: "aac".into(),
        };
        assert_eq!(db.match_track(71, &ja).unwrap(), Some(20));
        // one audio stream, and it is not the remembered one: still the track
        let only = TrackChoice {
            type_id: 2,
            language: "ja".into(),
            index: 9,
            codec: "aac".into(),
        };
        assert_eq!(db.match_track(72, &only).unwrap(), Some(30));
        // two audio streams and neither is the remembered one: do not guess
        assert_eq!(db.match_track(71, &only).unwrap(), None);
    }

    #[test]
    fn retarget_keeps_a_side_that_did_not_match() {
        let dir = tempfile::tempdir().unwrap();
        let db = db(dir.path());
        db.conn
            .execute(
                "INSERT INTO media_part_settings
                    (account_id, media_part_id, selected_audio_stream_id, selected_subtitle_stream_id)
                 VALUES (1, 70, 10, 11)",
                [],
            )
            .unwrap();
        db.retarget_selection(1, 70, Some(20), None).unwrap();
        let (audio, subtitle): (i64, i64) = db
            .conn
            .query_row(
                "SELECT selected_audio_stream_id, selected_subtitle_stream_id
                 FROM media_part_settings WHERE account_id=1 AND media_part_id=70",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((audio, subtitle), (20, 11));
    }

    #[test]
    fn a_remembered_choice_fills_an_empty_part_only() {
        let dir = tempfile::tempdir().unwrap();
        let db = db(dir.path());
        db.apply_remembered(1, 71, Some(20), None).unwrap();
        db.conn
            .execute(
                "INSERT INTO media_part_settings
                    (account_id, media_part_id, selected_audio_stream_id)
                 VALUES (2, 71, 21)",
                [],
            )
            .unwrap();
        db.apply_remembered(2, 71, Some(20), Some(99)).unwrap();
        let picked: i64 = db
            .conn
            .query_row(
                "SELECT selected_audio_stream_id FROM media_part_settings
                 WHERE account_id=2 AND media_part_id=71",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(picked, 21);
        let filled: i64 = db
            .conn
            .query_row(
                "SELECT selected_audio_stream_id FROM media_part_settings
                 WHERE account_id=1 AND media_part_id=71",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(filled, 20);
    }
}

/// One selected stream, as far as another part can recognise it.
///
/// Stream ids differ per part. Language and the stream's own index do not,
/// when the episodes were cut the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackChoice {
    /// 2 audio, 3 subtitle.
    pub type_id: i64,
    pub language: String,
    pub index: i64,
    pub codec: String,
}

/// The selection stored for one account on one part, plus the ids to put
/// back if the choice is undone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedSelection {
    pub account_id: i64,
    pub audio: Option<i64>,
    pub subtitle: Option<i64>,
}

impl Db {
    /// The show an episode belongs to.
    ///
    /// Episodes parent a season, and the season parents the show. Anything
    /// that is not an episode has no show here.
    pub fn show_of_item(&self, item_id: i64) -> rusqlite::Result<Option<i64>> {
        let show = self.conn.query_row(
            "SELECT show.id
             FROM metadata_items ep
             JOIN metadata_items season ON season.id = ep.parent_id
             JOIN metadata_items show ON show.id = season.parent_id
             WHERE ep.id = ? AND ep.metadata_type = 4 AND season.metadata_type = 3
               AND show.metadata_type = 2",
            [item_id],
            |row| row.get(0),
        );
        match show {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Every account's selected audio and subtitle on a part.
    ///
    /// A missing table (an older fixture, or a schema we do not write) is
    /// no selection, not an error: the rewrite still has to land.
    pub fn part_selections(&self, part_id: i64) -> rusqlite::Result<Vec<SavedSelection>> {
        let mut stmt = match self.conn.prepare_cached(
            "SELECT account_id, selected_audio_stream_id, selected_subtitle_stream_id
             FROM media_part_settings WHERE media_part_id = ?",
        ) {
            Ok(stmt) => stmt,
            Err(rusqlite::Error::SqliteFailure(err, Some(msg)))
                if err.code == rusqlite::ErrorCode::Unknown && msg.contains("no such table") =>
            {
                return Ok(Vec::new());
            }
            Err(e) => return Err(e),
        };
        let rows = stmt.query_map([part_id], |row| {
            Ok(SavedSelection {
                account_id: row.get(0)?,
                audio: row.get(1)?,
                subtitle: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    /// The stream a selected id names, when that row is still there.
    pub fn track_of_stream(&self, stream_id: i64) -> rusqlite::Result<Option<TrackChoice>> {
        let found = self
            .conn
            .prepare_cached(
                "SELECT stream_type_id, IFNULL(language, ''), IFNULL(\"index\", 0),
                        IFNULL(codec, '')
                 FROM media_streams WHERE id = ?",
            )?
            .query_row([stream_id], |row| {
                Ok(TrackChoice {
                    type_id: row.get(0)?,
                    language: row.get(1)?,
                    index: row.get(2)?,
                    codec: row.get(3)?,
                })
            })
            .optional()?;
        Ok(found)
    }

    /// The stream id on `part_id` that matches `choice`.
    ///
    /// Language and index win. A part that has no such stream, but has exactly
    /// one stream of that type, uses that one: a show whose later episode
    /// dropped the index still has the track the account picked.
    pub fn match_track(&self, part_id: i64, choice: &TrackChoice) -> rusqlite::Result<Option<i64>> {
        let exact: Option<i64> = self
            .conn
            .prepare_cached(
                "SELECT id FROM media_streams
                 WHERE media_part_id = ? AND stream_type_id = ?
                   AND IFNULL(language, '') = ? AND IFNULL(\"index\", 0) = ?
                 ORDER BY id LIMIT 1",
            )?
            .query_row(
                params![part_id, choice.type_id, choice.language, choice.index],
                |row| row.get(0),
            )
            .optional()?;
        if exact.is_some() {
            return Ok(exact);
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT id FROM media_streams WHERE media_part_id = ? AND stream_type_id = ?",
        )?;
        let ids: Vec<i64> = stmt
            .query_map(params![part_id, choice.type_id], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        if ids.len() == 1 {
            Ok(ids.first().copied())
        } else {
            Ok(None)
        }
    }

    /// Points one account's part settings at the streams that replaced the old
    /// ids. A side that did not match is left as it was.
    pub fn retarget_selection(
        &self,
        account_id: i64,
        part_id: i64,
        audio: Option<i64>,
        subtitle: Option<i64>,
    ) -> Result<(), WriteError> {
        if audio.is_none() && subtitle.is_none() {
            return Ok(());
        }
        let (known, version) = self.schema_known()?;
        if !known {
            return Err(WriteError::UnknownSchema(version));
        }
        if let Some(id) = audio {
            self.conn.execute(
                "UPDATE media_part_settings
                 SET selected_audio_stream_id = ?, updated_at = strftime('%s','now')
                 WHERE account_id = ? AND media_part_id = ?",
                params![id, account_id, part_id],
            )?;
        }
        if let Some(id) = subtitle {
            self.conn.execute(
                "UPDATE media_part_settings
                 SET selected_subtitle_stream_id = ?, updated_at = strftime('%s','now')
                 WHERE account_id = ? AND media_part_id = ?",
                params![id, account_id, part_id],
            )?;
        }
        Ok(())
    }

    /// Writes a remembered choice onto a part that has no selection yet.
    ///
    /// An account that already picked a stream on this part is left alone:
    /// the remembered choice fills the gap, it does not overwrite a pick.
    pub fn apply_remembered(
        &self,
        account_id: i64,
        part_id: i64,
        audio: Option<i64>,
        subtitle: Option<i64>,
    ) -> Result<(), WriteError> {
        if audio.is_none() && subtitle.is_none() {
            return Ok(());
        }
        let (known, version) = self.schema_known()?;
        if !known {
            return Err(WriteError::UnknownSchema(version));
        }
        let existing: Option<(Option<i64>, Option<i64>)> = self
            .conn
            .prepare_cached(
                "SELECT selected_audio_stream_id, selected_subtitle_stream_id
                 FROM media_part_settings WHERE account_id = ? AND media_part_id = ?",
            )?
            .query_row(params![account_id, part_id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()?;
        match existing {
            Some((have_audio, have_sub)) => {
                let audio = have_audio.or(audio);
                let subtitle = have_sub.or(subtitle);
                self.conn.execute(
                    "UPDATE media_part_settings
                     SET selected_audio_stream_id = ?, selected_subtitle_stream_id = ?,
                         updated_at = strftime('%s','now')
                     WHERE account_id = ? AND media_part_id = ?",
                    params![audio, subtitle, account_id, part_id],
                )?;
            }
            None => {
                self.conn.execute(
                    "INSERT INTO media_part_settings
                        (account_id, media_part_id, selected_audio_stream_id,
                         selected_subtitle_stream_id, created_at, updated_at)
                     VALUES (?, ?, ?, ?, strftime('%s','now'), strftime('%s','now'))",
                    params![account_id, part_id, audio, subtitle],
                )?;
            }
        }
        Ok(())
    }
}

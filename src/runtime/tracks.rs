//! Audio and subtitle choices, per account and per show.
//!
//! Plex stores a pick on the part, as stream ids. Those ids change when the
//! part is analysed again, and the next episode has ids of its own. What
//! stays is the language and the stream index. The previous ids are kept
//! beside the choice, so a rewrite can be undone.

use rusqlite::{OptionalExtension, Result, params};

use super::cache::{Cache, unix_seconds};
use crate::plexdb::TrackChoice;

/// What to store for one account on one show.
///
/// A side left as `None` keeps the side already stored, so learning the
/// audio does not forget the subtitle.
#[derive(Debug, Clone, Default)]
pub struct ChoiceUpdate {
    pub audio: Option<TrackChoice>,
    pub subtitle: Option<TrackChoice>,
    /// Selected stream ids on the part before they were rewritten. Recorded
    /// once: a later rewrite still restores the ids from before the first.
    pub previous_audio: Option<i64>,
    pub previous_subtitle: Option<i64>,
}

/// One remembered pick, and the stream ids it replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remembered {
    pub account_id: i64,
    pub show_id: i64,
    pub audio: Option<TrackChoice>,
    pub subtitle: Option<TrackChoice>,
    /// Selected stream ids on the part before they were rewritten.
    pub previous_audio: Option<i64>,
    pub previous_subtitle: Option<i64>,
}

pub(super) const TRACKS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS track_choices (
    account_id        INTEGER NOT NULL,
    show_id           INTEGER NOT NULL,
    audio_language    TEXT,
    audio_index       INTEGER,
    audio_codec       TEXT,
    subtitle_language TEXT,
    subtitle_index    INTEGER,
    subtitle_codec    TEXT,
    previous_audio    INTEGER,
    previous_subtitle INTEGER,
    updated_at        INTEGER NOT NULL,
    PRIMARY KEY (account_id, show_id)
)";

impl Cache {
    /// The choice stored for one account on one show.
    pub fn track_choice(&self, account_id: i64, show_id: i64) -> Result<Option<Remembered>> {
        self.conn()
            .prepare_cached(
                "SELECT audio_language, audio_index, audio_codec,
                        subtitle_language, subtitle_index, subtitle_codec,
                        previous_audio, previous_subtitle
                 FROM track_choices WHERE account_id=? AND show_id=?",
            )?
            .query_row(params![account_id, show_id], |row| {
                Ok(Remembered {
                    account_id,
                    show_id,
                    audio: choice_of(2, row.get(0)?, row.get(1)?, row.get(2)?),
                    subtitle: choice_of(3, row.get(3)?, row.get(4)?, row.get(5)?),
                    previous_audio: row.get(6)?,
                    previous_subtitle: row.get(7)?,
                })
            })
            .optional()
    }

    /// Remembers a choice. A side left empty keeps whatever was stored.
    pub fn remember_tracks(
        &self,
        account_id: i64,
        show_id: i64,
        update: &ChoiceUpdate,
        now: std::time::SystemTime,
    ) -> Result<()> {
        let existing = self.track_choice(account_id, show_id)?;
        let audio = update
            .audio
            .as_ref()
            .or(existing.as_ref().and_then(|row| row.audio.as_ref()));
        let subtitle = update
            .subtitle
            .as_ref()
            .or(existing.as_ref().and_then(|row| row.subtitle.as_ref()));
        let previous_audio = existing
            .as_ref()
            .and_then(|row| row.previous_audio)
            .or(update.previous_audio);
        let previous_subtitle = existing
            .as_ref()
            .and_then(|row| row.previous_subtitle)
            .or(update.previous_subtitle);
        self.conn()
            .prepare_cached(
                "INSERT INTO track_choices (
                    account_id, show_id,
                    audio_language, audio_index, audio_codec,
                    subtitle_language, subtitle_index, subtitle_codec,
                    previous_audio, previous_subtitle, updated_at
                 ) VALUES (?,?,?,?,?,?,?,?,?,?,?)
                 ON CONFLICT(account_id, show_id) DO UPDATE SET
                    audio_language=excluded.audio_language,
                    audio_index=excluded.audio_index,
                    audio_codec=excluded.audio_codec,
                    subtitle_language=excluded.subtitle_language,
                    subtitle_index=excluded.subtitle_index,
                    subtitle_codec=excluded.subtitle_codec,
                    previous_audio=excluded.previous_audio,
                    previous_subtitle=excluded.previous_subtitle,
                    updated_at=excluded.updated_at",
            )?
            .execute(params![
                account_id,
                show_id,
                audio.map(|c| c.language.clone()),
                audio.map(|c| c.index),
                audio.map(|c| c.codec.clone()),
                subtitle.map(|c| c.language.clone()),
                subtitle.map(|c| c.index),
                subtitle.map(|c| c.codec.clone()),
                previous_audio,
                previous_subtitle,
                unix_seconds(now),
            ])?;
        Ok(())
    }

    /// Accounts that have a remembered choice for this show.
    pub fn accounts_for_show(&self, show_id: i64) -> Result<Vec<i64>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare_cached("SELECT account_id FROM track_choices WHERE show_id=?")?;
        stmt.query_map([show_id], |row| row.get(0))?.collect()
    }

    /// Forgets the remembered choice. The caller puts the previous ids back
    /// on the part; this only drops the ledger row.
    pub fn clear_track_choice(&self, account_id: i64, show_id: i64) -> Result<Option<Remembered>> {
        let row = self.track_choice(account_id, show_id)?;
        if row.is_some() {
            self.conn()
                .prepare_cached("DELETE FROM track_choices WHERE account_id=? AND show_id=?")?
                .execute(params![account_id, show_id])?;
        }
        Ok(row)
    }
}

/// A stored side. A missing language means that side was never chosen.
fn choice_of(
    type_id: i64,
    language: Option<String>,
    index: Option<i64>,
    codec: Option<String>,
) -> Option<TrackChoice> {
    Some(TrackChoice {
        type_id,
        language: language?,
        index: index.unwrap_or(0),
        codec: codec.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::cache::tests::test_cache;

    fn audio(language: &str, index: i64) -> TrackChoice {
        TrackChoice {
            type_id: 2,
            language: language.into(),
            index,
            codec: "aac".into(),
        }
    }

    #[test]
    fn a_choice_is_remembered_per_account_and_show() {
        let (_dir, cache) = test_cache();
        let now = std::time::SystemTime::now();
        let picked = audio("ja", 2);
        cache
            .remember_tracks(
                1,
                9,
                &ChoiceUpdate {
                    audio: Some(picked),
                    previous_audio: Some(40),
                    ..Default::default()
                },
                now,
            )
            .unwrap();
        let row = cache.track_choice(1, 9).unwrap().unwrap();
        assert_eq!(row.audio.as_ref().map(|c| c.language.as_str()), Some("ja"));
        assert_eq!(row.audio.as_ref().map(|c| c.index), Some(2));
        assert!(row.subtitle.is_none());
        assert_eq!(row.previous_audio, Some(40));
        assert!(cache.track_choice(2, 9).unwrap().is_none());
        assert!(cache.track_choice(1, 8).unwrap().is_none());
    }

    #[test]
    fn a_later_side_does_not_drop_the_first() {
        let (_dir, cache) = test_cache();
        let now = std::time::SystemTime::now();
        let audio = audio("en", 1);
        let subtitle = TrackChoice {
            type_id: 3,
            language: "zh".into(),
            index: 4,
            codec: "srt".into(),
        };
        cache
            .remember_tracks(
                1,
                9,
                &ChoiceUpdate {
                    audio: Some(audio),
                    previous_audio: Some(40),
                    previous_subtitle: Some(41),
                    ..Default::default()
                },
                now,
            )
            .unwrap();
        cache
            .remember_tracks(
                1,
                9,
                &ChoiceUpdate {
                    subtitle: Some(subtitle),
                    previous_audio: Some(90),
                    ..Default::default()
                },
                now,
            )
            .unwrap();
        let row = cache.track_choice(1, 9).unwrap().unwrap();
        assert_eq!(row.audio.unwrap().language, "en");
        assert_eq!(row.subtitle.unwrap().language, "zh");
        // the first previous ids stay: a second rewrite must still restore them
        assert_eq!(row.previous_audio, Some(40));
        assert_eq!(row.previous_subtitle, Some(41));
    }

    #[test]
    fn clearing_returns_the_row_and_forgets_it() {
        let (_dir, cache) = test_cache();
        let now = std::time::SystemTime::now();
        let picked = audio("en", 1);
        cache
            .remember_tracks(
                1,
                9,
                &ChoiceUpdate {
                    audio: Some(picked),
                    previous_audio: Some(7),
                    ..Default::default()
                },
                now,
            )
            .unwrap();
        let cleared = cache.clear_track_choice(1, 9).unwrap().unwrap();
        assert_eq!(cleared.previous_audio, Some(7));
        assert!(cache.track_choice(1, 9).unwrap().is_none());
        assert!(cache.clear_track_choice(1, 9).unwrap().is_none());
    }
}

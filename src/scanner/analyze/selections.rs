//! Keeps audio and subtitle choices across a rewrite of a part's streams.
//!
//! A rewrite replaces the stream rows, which drops the ids a saved choice
//! pointed at. The choices are read first, named by language and index,
//! then pointed at the new rows. A choice remembered for the account and
//! show fills a part the account has not picked on yet.

use std::collections::HashSet;
use std::time::SystemTime;

use crate::plexdb::{Db, TrackChoice};
use crate::runtime::{Cache, ChoiceUpdate};

/// The selections on a part, named by language and index so they survive
/// the stream rows being replaced.
pub(super) fn keep_selections(db: &Db, part_id: i64) -> Vec<Kept> {
    let Ok(rows) = db.part_selections(part_id) else {
        return Vec::new();
    };
    rows.into_iter()
        .map(|row| Kept {
            account_id: row.account_id,
            audio: row
                .audio
                .and_then(|id| db.track_of_stream(id).ok().flatten()),
            subtitle: row
                .subtitle
                .and_then(|id| db.track_of_stream(id).ok().flatten()),
            previous_audio: row.audio,
            previous_subtitle: row.subtitle,
        })
        .collect()
}

pub(super) struct Kept {
    account_id: i64,
    audio: Option<TrackChoice>,
    subtitle: Option<TrackChoice>,
    previous_audio: Option<i64>,
    previous_subtitle: Option<i64>,
}

/// Points each saved selection at the stream that replaced it, and remembers
/// the choice for the show.
pub(super) fn retarget_kept(
    db: &Db,
    part_id: i64,
    show: Option<i64>,
    kept: &[Kept],
    cache: Option<&Cache>,
) {
    for row in kept {
        let audio = matched(db, part_id, row.audio.as_ref());
        let subtitle = matched(db, part_id, row.subtitle.as_ref());
        let _ = db.retarget_selection(row.account_id, part_id, audio, subtitle);
        if let (Some(cache), Some(show)) = (cache, show) {
            let _ = cache.remember_tracks(
                row.account_id,
                show,
                &ChoiceUpdate {
                    audio: row.audio.clone(),
                    subtitle: row.subtitle.clone(),
                    previous_audio: row.previous_audio,
                    previous_subtitle: row.previous_subtitle,
                },
                SystemTime::now(),
            );
        }
    }
}

/// Applies a choice remembered on another episode of this show.
///
/// An account that already had a row on this part was retargeted from that
/// row. This only fills a part the account has not picked on yet.
pub(super) fn apply_show_choice(db: &Db, cache: &Cache, show: i64, part_id: i64, kept: &[Kept]) {
    let already: HashSet<i64> = kept.iter().map(|row| row.account_id).collect();
    let Ok(accounts) = cache.accounts_for_show(show) else {
        return;
    };
    for account in accounts {
        if already.contains(&account) {
            continue;
        }
        let Ok(Some(remembered)) = cache.track_choice(account, show) else {
            continue;
        };
        let audio = matched(db, part_id, remembered.audio.as_ref());
        let subtitle = matched(db, part_id, remembered.subtitle.as_ref());
        let _ = db.apply_remembered(account, part_id, audio, subtitle);
    }
}

fn matched(db: &Db, part_id: i64, choice: Option<&TrackChoice>) -> Option<i64> {
    db.match_track(part_id, choice?).ok().flatten()
}

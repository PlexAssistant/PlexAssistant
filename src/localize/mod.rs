//! Optional Chinese sort titles, tag labels and cast names for fields Plex
//! already stored.
//!
//! Off unless `ZH_LOCALIZE` names the work. `sort` writes a sort title.
//! `tags` rewrites genre, mood and style labels the table knows. `people`
//! rewrites director, writer and actor names from the credits of the item's
//! TMDB id. A field that was written is locked, so turning the switch off
//! leaves it. `ZH_LOCALIZE_UNLOCK` drops those locks and does not write; the
//! next metadata refresh can put the source values back.

mod credits;
mod pinyin;
mod tags;

use std::collections::BTreeSet;

/// `ZH_LOCALIZE`. Unset means nothing is rewritten.
pub const ENV: &str = "ZH_LOCALIZE";

/// `ZH_LOCALIZE_UNLOCK`. Set to drop the locks this pass wrote.
pub const UNLOCK_ENV: &str = "ZH_LOCALIZE_UNLOCK";

/// `TMDB_API_KEY`. Set to use another v3 key. Unset uses [`DEFAULT_TMDB_KEY`].
pub const TMDB_KEY_ENV: &str = "TMDB_API_KEY";

/// The v3 key used when [`TMDB_KEY_ENV`] is unset.
pub const DEFAULT_TMDB_KEY: &str = "db55323b8d3e4154498498a75642b381";

/// Whether a scan should rewrite sort titles, tags or cast names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Localize {
    pub sort: bool,
    pub tags: bool,
    pub people: bool,
}

impl Localize {
    /// `sort`, `tags` or `people` anywhere in the comma-separated list turns
    /// that rewrite on. Any other name is ignored.
    pub fn parse(value: Option<&str>) -> Self {
        let named = |want: &str| {
            value.is_some_and(|value| value.split(',').any(|name| name.trim() == want))
        };
        Self {
            sort: named("sort"),
            tags: named("tags"),
            people: named("people"),
        }
    }

    /// The choice named by [`ENV`].
    pub fn from_env() -> Self {
        Self::parse(std::env::var(ENV).ok().as_deref())
    }
}

/// Whether `value` asks for the locks to be dropped. Empty is off. `0` is
/// off. Anything else is on, so a bare `ZH_LOCALIZE_UNLOCK=1` works.
pub fn unlock_from(value: Option<&str>) -> bool {
    match value.map(str::trim) {
        None | Some("") | Some("0") => false,
        Some(_) => true,
    }
}

/// The choice named by [`UNLOCK_ENV`].
pub fn unlock_from_env() -> bool {
    unlock_from(std::env::var(UNLOCK_ENV).ok().as_deref())
}

/// The v3 key named by [`TMDB_KEY_ENV`], or [`DEFAULT_TMDB_KEY`] when that is
/// unset or blank.
pub fn tmdb_key_from_env() -> String {
    tmdb_key_from(std::env::var(TMDB_KEY_ENV).ok().as_deref())
}

/// A trimmed key. Missing or blank uses [`DEFAULT_TMDB_KEY`].
pub fn tmdb_key_from(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| DEFAULT_TMDB_KEY.to_owned())
}

/// One person on a credits list, named in the requested language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credit {
    pub name: String,
    /// The name in its original language. A library row still holding that
    /// name is the same person.
    pub original: String,
}

/// Directors, writers and cast of one film or episode.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Credits {
    pub directors: Vec<Credit>,
    pub writers: Vec<Credit>,
    pub cast: Vec<Credit>,
}

/// Which credits list to ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreditTarget {
    /// `/3/movie/{id}/credits`.
    Movie(u64),
    /// `/3/tv/{id}/credits`, for a show that stores its cast on the show.
    Show(u64),
    /// `/3/tv/{id}/season/{season}/episode/{episode}/credits`.
    Episode { id: u64, season: u32, episode: u32 },
}

/// The credits of `target` in Chinese, or the original name where no Chinese
/// name is returned. A missing key, a bad status or a body that is not
/// credits returns nothing, so the scan can skip the item.
pub async fn fetch_credits(key: &str, target: CreditTarget) -> Option<Credits> {
    credits::fetch(key, target).await
}

/// The Chinese label for an English genre, mood or style tag, if the table
/// has one. A label that is already Chinese is not in the table.
pub fn translate_tag(label: &str) -> Option<&'static str> {
    tags::translate(label)
}

/// The sort title for `title`, or nothing when it should be left as stored.
/// Punctuation is dropped. A title that mixes Han with kana is skipped.
pub fn sort_title(title: &str) -> Option<String> {
    pinyin::initials(title)
}

/// Field ids Plex stores in `metadata_items.user_fields` as
/// `lockedFields=<id>|<id>`. Measured against Plex 1.43.5: locking `titleSort`
/// wrote `2`. Locking `mood` wrote `300`, `style` wrote `301`.
pub const LOCK_TITLE_SORT: i64 = 2;
pub const LOCK_GENRE: i64 = 1;
pub const LOCK_MOOD: i64 = 300;
pub const LOCK_STYLE: i64 = 301;

/// The lock ids the tag rewrite owns.
pub const TAG_LOCKS: [i64; 3] = [LOCK_GENRE, LOCK_MOOD, LOCK_STYLE];

/// Field ids for the cast rewrite. Measured against Plex 1.43.5: locking
/// `director` wrote `17`, `writer` wrote `18`, `actor` wrote `19`.
pub const LOCK_DIRECTOR: i64 = 17;
pub const LOCK_WRITER: i64 = 18;
pub const LOCK_ACTOR: i64 = 19;

/// The lock ids the cast rewrite owns.
pub const PEOPLE_LOCKS: [i64; 3] = [LOCK_DIRECTOR, LOCK_WRITER, LOCK_ACTOR];

/// `user_fields` with `id` added to the `lockedFields` list. Other keys in
/// the column are kept. Adding an id that is already there changes nothing.
pub fn lock_field(user_fields: &str, id: i64) -> String {
    let (mut ids, rest) = split_locks(user_fields);
    ids.insert(id);
    join_locks(&ids, &rest)
}

/// `user_fields` with `id` removed from the `lockedFields` list. Other keys
/// are kept. An empty lock list is dropped, so a column that only held these
/// locks goes back to empty.
pub fn unlock_field(user_fields: &str, id: i64) -> String {
    let (mut ids, rest) = split_locks(user_fields);
    ids.remove(&id);
    join_locks(&ids, &rest)
}

fn split_locks(user_fields: &str) -> (BTreeSet<i64>, Vec<&str>) {
    // The column joins every piece with '|', and the prefix is only on the
    // first id: "lockedFields=1|2|pv:x=1". Bare numbers after that prefix are
    // more lock ids. Anything else is kept as its own key.
    let mut ids = BTreeSet::new();
    let mut rest = Vec::new();
    let mut in_locks = false;
    for part in user_fields.split('|').filter(|p| !p.is_empty()) {
        if let Some(list) = part.strip_prefix("lockedFields=") {
            in_locks = true;
            // "lockedFields=" with nothing after it is how Plex stores an
            // empty list. It names no field, so it is not kept.
            if let Ok(id) = list.parse::<i64>() {
                ids.insert(id);
            }
            continue;
        }
        if in_locks && let Ok(id) = part.parse::<i64>() {
            ids.insert(id);
            continue;
        }
        in_locks = false;
        rest.push(part);
    }
    (ids, rest)
}

fn join_locks(ids: &BTreeSet<i64>, rest: &[&str]) -> String {
    let mut out = String::new();
    if !ids.is_empty() {
        out.push_str("lockedFields=");
        out.push_str(
            &ids.iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("|"),
        );
    }
    if !rest.is_empty() {
        if !out.is_empty() {
            out.push('|');
        }
        out.push_str(&rest.join("|"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_selects_nothing() {
        assert_eq!(Localize::parse(None), Localize::default());
        assert!(!Localize::parse(Some("")).sort);
        assert!(!Localize::parse(Some("")).tags);
        assert!(!Localize::parse(Some("")).people);
        assert!(Localize::parse(Some("people")).people);
        assert!(!Localize::parse(Some("people")).sort);
        assert!(!Localize::parse(Some("people")).tags);
    }

    #[test]
    fn names_are_independent() {
        let both = Localize::parse(Some("sort, tags, people"));
        assert!(both.sort);
        assert!(both.tags);
        assert!(both.people);
        assert!(Localize::parse(Some("sort")).sort);
        assert!(!Localize::parse(Some("sort")).tags);
        assert!(!Localize::parse(Some("sort")).people);
    }

    #[test]
    fn a_blank_key_uses_the_default() {
        assert_eq!(tmdb_key_from(None), DEFAULT_TMDB_KEY);
        assert_eq!(tmdb_key_from(Some("")), DEFAULT_TMDB_KEY);
        assert_eq!(tmdb_key_from(Some("  ")), DEFAULT_TMDB_KEY);
        assert_eq!(tmdb_key_from(Some(" abc ")), "abc");
    }

    #[test]
    fn sort_title_uses_initials() {
        assert_eq!(sort_title("重庆森林").as_deref(), Some("CQSL"));
        assert_eq!(sort_title("鬼滅の刃"), None);
    }

    #[test]
    fn unlock_is_off_until_set() {
        assert!(!unlock_from(None));
        assert!(!unlock_from(Some("")));
        assert!(!unlock_from(Some("0")));
        assert!(unlock_from(Some("1")));
    }

    #[test]
    fn tag_table_covers_the_sample() {
        assert_eq!(translate_tag("Action"), Some("动作"));
        assert_eq!(translate_tag("Sad"), Some("悲伤"));
        assert_eq!(translate_tag("剧情"), None);
    }

    #[test]
    fn locking_keeps_other_fields() {
        assert_eq!(lock_field("", LOCK_GENRE), "lockedFields=1");
        assert_eq!(lock_field("lockedFields=2", LOCK_GENRE), "lockedFields=1|2");
        assert_eq!(
            lock_field("lockedFields=2|pv:x=1", LOCK_GENRE),
            "lockedFields=1|2|pv:x=1"
        );
        assert_eq!(lock_field("lockedFields=1", LOCK_GENRE), "lockedFields=1");
    }

    #[test]
    fn unlocking_drops_only_that_field() {
        assert_eq!(
            unlock_field("lockedFields=1|2", LOCK_GENRE),
            "lockedFields=2"
        );
        assert_eq!(unlock_field("lockedFields=1", LOCK_GENRE), "");
        assert_eq!(unlock_field("lockedFields=1|pv:x=1", LOCK_GENRE), "pv:x=1");
        assert_eq!(unlock_field("lockedFields=", LOCK_GENRE), "");
    }
}

//! Optional Chinese labels for genre, mood and style tags Plex already stored.
//!
//! Off unless `ZH_LOCALIZE` contains `tags`. A tag the table does not know
//! is left as stored. A label that was written is locked, so turning the
//! switch off leaves it. `ZH_LOCALIZE_UNLOCK` drops those locks and does not
//! write labels; the next metadata refresh can put the source values back.

mod tags;

use std::collections::BTreeSet;

/// `ZH_LOCALIZE`. Unset means nothing is rewritten.
pub const ENV: &str = "ZH_LOCALIZE";

/// `ZH_LOCALIZE_UNLOCK`. Set to drop the locks this pass wrote.
pub const UNLOCK_ENV: &str = "ZH_LOCALIZE_UNLOCK";

/// Whether a scan should rewrite tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Localize {
    pub tags: bool,
}

impl Localize {
    /// `tags` anywhere in the comma-separated list turns the rewrite on.
    /// Any other name is ignored.
    pub fn parse(value: Option<&str>) -> Self {
        let tags = value.is_some_and(|value| {
            value
                .split(',')
                .any(|name| name.trim() == "tags")
        });
        Self { tags }
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

/// The Chinese label for an English genre, mood or style tag, if the table
/// has one. A label that is already Chinese is not in the table.
pub fn translate_tag(label: &str) -> Option<&'static str> {
    tags::translate(label)
}

/// Field ids Plex stores in `metadata_items.user_fields` as
/// `lockedFields=<id>|<id>`. Measured against Plex 1.43.5: locking `genre`
/// wrote `1`, `mood` wrote `300`, `style` wrote `301`.
pub const LOCK_GENRE: i64 = 1;
pub const LOCK_MOOD: i64 = 300;
pub const LOCK_STYLE: i64 = 301;

/// The lock ids this pass owns.
pub const LOCKS: [i64; 3] = [LOCK_GENRE, LOCK_MOOD, LOCK_STYLE];

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
        assert!(!Localize::parse(Some("")).tags);
        assert!(!Localize::parse(Some("sort,people")).tags);
    }

    #[test]
    fn tags_is_the_only_name() {
        assert!(Localize::parse(Some("tags")).tags);
        assert!(Localize::parse(Some("sort, tags")).tags);
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
        assert_eq!(unlock_field("lockedFields=1|2", LOCK_GENRE), "lockedFields=2");
        assert_eq!(unlock_field("lockedFields=1", LOCK_GENRE), "");
        assert_eq!(
            unlock_field("lockedFields=1|pv:x=1", LOCK_GENRE),
            "pv:x=1"
        );
    }
}

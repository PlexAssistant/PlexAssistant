//! Optional Chinese sort titles and tag labels for fields Plex already stored.
//!
//! Off unless `ZH_LOCALIZE` names the work. `sort` writes a sort title.
//! `tags` rewrites genre, mood and style labels the table knows. A field that
//! was written is locked, so turning the switch off leaves it.
//! `ZH_LOCALIZE_UNLOCK` drops those locks and does not write; the next
//! metadata refresh can put the source values back.

mod pinyin;
mod tags;

use std::collections::BTreeSet;

/// `ZH_LOCALIZE`. Unset means nothing is rewritten.
pub const ENV: &str = "ZH_LOCALIZE";

/// `ZH_LOCALIZE_UNLOCK`. Set to drop the locks this pass wrote.
pub const UNLOCK_ENV: &str = "ZH_LOCALIZE_UNLOCK";

/// Whether a scan should rewrite sort titles or tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Localize {
    pub sort: bool,
    pub tags: bool,
}

impl Localize {
    /// `sort` or `tags` anywhere in the comma-separated list turns that
    /// rewrite on. Any other name is ignored.
    pub fn parse(value: Option<&str>) -> Self {
        let named = |want: &str| {
            value.is_some_and(|value| value.split(',').any(|name| name.trim() == want))
        };
        Self {
            sort: named("sort"),
            tags: named("tags"),
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
        assert!(!Localize::parse(Some("")).sort);
        assert!(!Localize::parse(Some("")).tags);
        assert!(!Localize::parse(Some("people")).sort);
        assert!(!Localize::parse(Some("people")).tags);
    }

    #[test]
    fn names_are_independent() {
        let both = Localize::parse(Some("sort, tags"));
        assert!(both.sort);
        assert!(both.tags);
        assert!(Localize::parse(Some("sort")).sort);
        assert!(!Localize::parse(Some("sort")).tags);
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
    }
}

//! English genre, mood and style labels, and the Chinese Plex already shows
//! for them. A label that is not in the table is left as stored.
//!
//! The table is generated from `scripts/codegen/tags.json`. Genre labels
//! follow chinese-localization-for-plex; mood and style labels live in the
//! same file because Plex stores them as the same string on a different tag
//! type.

#[path = "tag_table.rs"]
mod tag_table;

use tag_table::LABELS;

pub(super) fn translate(label: &str) -> Option<&'static str> {
    LABELS
        .binary_search_by_key(&label, |&(en, _)| en)
        .ok()
        .map(|i| LABELS[i].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_and_unique() {
        assert!(LABELS.windows(2).all(|w| w[0].0 < w[1].0));
    }
}

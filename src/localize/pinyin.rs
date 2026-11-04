//! Sort titles as pinyin initials. The display title is not touched.
//!
//! A title that mixes Han with kana is left alone: the reading of the Han
//! would sort the Japanese title wrong. Punctuation is dropped. ASCII letters
//! and digits stay, uppercased. `重庆森林` sorts as `CQSL`.

use pinyin_pro::options::{PatternKind, PinyinOptions, PinyinOutput};
use pinyin_pro::pinyin;

/// The sort title for `title`, or nothing when it should be left as stored.
pub(super) fn initials(title: &str) -> Option<String> {
    let title = title.trim();
    if title.is_empty() || has_kana(title) {
        return None;
    }
    let mut out = String::new();
    let mut han = String::new();
    for ch in title.chars().chain(['\0']) {
        if is_han(ch) {
            han.push(ch);
            continue;
        }
        if !han.is_empty() {
            out.push_str(&han_initials(&han));
            han.clear();
        }
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_uppercase());
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Initials of one Han run, so a word such as 重庆 is read as a word.
fn han_initials(han: &str) -> String {
    let options = PinyinOptions {
        pattern: PatternKind::First,
        separator: String::new(),
        ..PinyinOptions::default()
    };
    let PinyinOutput::Str(raw) = pinyin(han, options) else {
        return String::new();
    };
    raw.chars()
        .filter(|c| c.is_ascii_alphabetic())
        .flat_map(char::to_uppercase)
        .collect()
}

/// Han plus kana means a Japanese title. The middle-dot used between names
/// is not kana, so it does not by itself skip a title.
fn has_kana(title: &str) -> bool {
    let mut han = false;
    let mut kana = false;
    for ch in title.chars() {
        han |= is_han(ch);
        kana |= is_kana(ch);
        if han && kana {
            return true;
        }
    }
    false
}

fn is_han(ch: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&ch)
}

fn is_kana(ch: char) -> bool {
    ('\u{3040}'..='\u{30ff}').contains(&ch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chongqing_forest_sorts_as_initials() {
        assert_eq!(initials("重庆森林").as_deref(), Some("CQSL"));
    }

    #[test]
    fn punctuation_is_dropped_and_ascii_kept() {
        assert_eq!(initials("冥河（上）").as_deref(), Some("MHS"));
        assert_eq!(initials("S1 行尸").as_deref(), Some("S1XS"));
    }

    #[test]
    fn kana_titles_are_skipped() {
        assert_eq!(initials("進撃の巨人"), None);
        assert_eq!(initials("鬼滅の刃"), None);
    }

    #[test]
    fn a_middle_dot_does_not_skip() {
        assert_eq!(initials("让·雷诺").as_deref(), Some("RLN"));
    }

    #[test]
    fn nothing_to_sort_is_skipped() {
        assert_eq!(initials(""), None);
        assert_eq!(initials("   "), None);
        assert_eq!(initials("（）"), None);
    }
}

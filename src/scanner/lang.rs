use super::lang_table::ISO639_2_TO_1;

/// Maps an ffprobe language tag to what Plex stores: the ISO 639-1 code
/// where one exists ("eng"/"en" -> "en", "chi"/"zho" -> "zh"), any other
/// code as-is ("fil"), "" for "und", and for an untagged stream the
/// container's default: English in Matroska (its spec default), none
/// elsewhere. Verified against Plex 1.43.5 (tests/fixtures).
pub(super) fn normalize_lang(tag: &str, matroska: bool) -> String {
    let tag = tag.trim().to_ascii_lowercase();
    match tag.as_str() {
        "" if matroska => "en".to_owned(),
        "" | "und" => String::new(),
        three if three.len() == 3 => ISO639_2_TO_1
            .binary_search_by_key(&three, |&(t, _)| t)
            .map_or(tag.clone(), |i| ISO639_2_TO_1[i].1.to_owned()),
        _ => tag,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted() {
        assert!(ISO639_2_TO_1.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn like_plex() {
        // (tag, matroska, stored) as pinned by tests/fixtures/plex-1.43.5.11079
        for (tag, matroska, want) in [
            ("en", true, "en"),
            ("eng", true, "en"),
            ("jpn", true, "ja"),
            ("chi", true, "zh"),
            ("zho", true, "zh"),
            ("fre", true, "fr"),
            ("ger", true, "de"),
            ("por", true, "pt"),
            ("kor", true, "ko"),
            ("de", true, "de"),
            ("fil", true, "fil"),
            ("", true, "en"),
            ("", false, ""),
            ("und", false, ""),
            ("und", true, ""),
            ("FRE", false, "fr"),
            ("pt-br", false, "pt-br"),
        ] {
            assert_eq!(
                normalize_lang(tag, matroska),
                want,
                "{tag:?} matroska={matroska}"
            );
        }
    }
}

//! Keeps signed CDN tokens out of the log.

/// Strips query strings from http(s) URLs embedded in text destined for
/// the log: ffmpeg banners and errors echo the (signed) input URL, and
/// the query is where the CDN token lives. Everything else is passed
/// through untouched.
pub fn redact_urls(text: &str) -> String {
    fn next_url(text: &str) -> Option<usize> {
        ["http://", "https://"]
            .iter()
            .filter_map(|m| text.find(m))
            .min()
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = next_url(rest) {
        let (before, after) = rest.split_at(pos);
        out.push_str(before);
        let end = after.find(char::is_whitespace).unwrap_or(after.len());
        let url = &after[..end];
        match url.find(['?', '#']) {
            Some(cut) => {
                out.push_str(&url[..cut]);
                out.push('…');
            }
            None => out.push_str(url),
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_query_but_keeps_path() {
        assert_eq!(
            redact_urls("GET https://cdn.example/file.mkv?sign=abc123&exp=99 done"),
            "GET https://cdn.example/file.mkv… done"
        );
    }

    #[test]
    fn leaves_plain_text_and_bare_urls_alone() {
        assert_eq!(redact_urls("no urls here"), "no urls here");
        assert_eq!(
            redact_urls("from 'https://cdn.example/file.mkv'"),
            "from 'https://cdn.example/file.mkv'"
        );
    }

    #[test]
    fn handles_several_urls_and_line_ends() {
        assert_eq!(
            redact_urls("a http://x/1?t=1 b\nhttps://y/2?sig=zz tail"),
            "a http://x/1… b\nhttps://y/2… tail"
        );
    }
}

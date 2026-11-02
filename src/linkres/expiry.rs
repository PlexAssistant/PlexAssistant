use std::time::{SystemTime, UNIX_EPOCH};

use url::Url;
use url::form_urlencoded;

/// Looks for a plausible unix-seconds timestamp in the URL query (signed
/// links commonly carry exp/expires/t=<exp>.<sig>). It is informational
/// only; 0 means unknown.
pub fn sniff_expiry(raw_url: &str, now: SystemTime) -> i64 {
    let Ok(url) = Url::parse(raw_url) else {
        return 0;
    };
    let now = unix_seconds(now);
    url.query()
        .unwrap_or_default()
        .split('&')
        // Go's url.Query drops whole segments containing ';'; keep parity.
        .filter(|segment| !segment.contains(';'))
        .flat_map(|segment| form_urlencoded::parse(segment.as_bytes()))
        .filter_map(|(_, value)| {
            let digits = leading_digits(&value);
            if digits.len() != 10 {
                return None; // signatures are longer or hex
            }
            digits.parse::<i64>().ok().filter(|&n| n > now)
        })
        .max()
        .unwrap_or(0)
}

/// Returns the run of ASCII digits at the start of s.
fn leading_digits(s: &str) -> &str {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    &s[..end]
}

pub(crate) fn unix_seconds(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn sniff() {
        let now = SystemTime::now();
        let future = unix_seconds(now + Duration::from_secs(3600));
        let past = unix_seconds(now - Duration::from_secs(3600));

        let cases = [
            (
                "signed token",
                format!("http://x/dl/a?t={future}.deadbeef"),
                future,
            ),
            ("plain exp", format!("http://x/dl/a?exp={future}"), future),
            ("past ignored", format!("http://x/dl/a?t={past}"), 0),
            ("no timestamp", "http://x/dl/a?sig=abcdef1234".into(), 0),
            ("wrong length", format!("http://x/dl/a?t={future}9"), 0),
            (
                "latest wins",
                format!("http://x/dl/a?a={now_}&b={future}", now_ = future - 60),
                future,
            ),
            (
                "semicolon segment skipped",
                format!("http://x/dl/a?exp={future};x"),
                0,
            ),
            ("no query", "http://x/dl/a".into(), 0),
            ("not a url", "::".into(), 0),
        ];
        for (name, url, want) in cases {
            assert_eq!(sniff_expiry(&url, now), want, "{name}: {url}");
        }
    }
}

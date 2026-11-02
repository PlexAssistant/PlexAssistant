//! Lenient number parsing for ffprobe's string fields: anything malformed
//! reads as 0, like the Go helpers.

/// Turns ffprobe rates like "24/1" or "24000/1001" into a float.
pub(super) fn parse_fps(v: &str) -> f64 {
    let Some((num, den)) = v.split_once('/') else {
        return atof(v);
    };
    match atof(den) {
        0.0 => 0.0,
        den => atof(num) / den,
    }
}

pub(super) fn atof(s: &str) -> f64 {
    s.trim().parse().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fps() {
        assert_eq!(parse_fps("24/1"), 24.0);
        assert_eq!((parse_fps("24000/1001") * 1000.0) as i64, 23976);
        assert_eq!(parse_fps("25"), 25.0);
        assert_eq!(parse_fps("0/0"), 0.0);
        assert_eq!(parse_fps("junk"), 0.0);
    }
}

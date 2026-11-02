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

/// Turns "16:9" into 1.777...
pub(super) fn parse_ratio(v: &str) -> f64 {
    let Some((a, b)) = v.split_once(':') else {
        return 0.0;
    };
    match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(x), Ok(y)) if y != 0.0 => x / y,
        _ => 0.0,
    }
}

pub(super) fn atof(s: &str) -> f64 {
    s.trim().parse().unwrap_or(0.0)
}

pub(super) fn atoi64(s: &str) -> i64 {
    s.trim().parse().unwrap_or(0)
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

    #[test]
    fn ratio() {
        assert!((1.777..1.778).contains(&parse_ratio("16:9")));
        assert_eq!(parse_ratio("4:0"), 0.0);
        assert_eq!(parse_ratio("bogus"), 0.0);
        assert_eq!(parse_ratio("a:b"), 0.0);
    }

    #[test]
    fn integers() {
        assert_eq!(atoi64(" 1501731 "), 1501731);
        assert_eq!(atoi64("N/A"), 0);
        assert_eq!(atoi64(""), 0);
    }
}

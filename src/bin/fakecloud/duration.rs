//! Go time.Duration text: the rig scripts and /state speak it ("10s",
//! "1h30m", "1h0m0s"), so parsing and formatting follow Go exactly.

const NS: u128 = 1;
const US: u128 = 1_000;
const MS: u128 = 1_000_000;
const SEC: u128 = 1_000_000_000;

/// Parses like time.ParseDuration into signed nanoseconds.
pub fn parse(s: &str) -> Result<i64, String> {
    let err = || format!("invalid duration {s:?}");
    let (neg, mut rest) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    if rest == "0" {
        return Ok(0);
    }
    if rest.is_empty() {
        return Err(err());
    }
    let mut total: u128 = 0;
    while !rest.is_empty() {
        let int_len = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let (int, after) = rest.split_at(int_len);
        let (frac, after) = match after.strip_prefix('.') {
            Some(after) => {
                let len = after
                    .find(|c: char| !c.is_ascii_digit())
                    .unwrap_or(after.len());
                after.split_at(len)
            }
            None => ("", after),
        };
        if int.is_empty() && frac.is_empty() {
            return Err(err());
        }
        let unit_len = after
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(after.len());
        let (unit, after) = after.split_at(unit_len);
        let unit = match unit {
            "ns" => NS,
            "us" | "µs" | "μs" => US,
            "ms" => MS,
            "s" => SEC,
            "m" => 60 * SEC,
            "h" => 3600 * SEC,
            _ => return Err(err()),
        };
        let int: u128 = if int.is_empty() {
            0
        } else {
            int.parse().map_err(|_| err())?
        };
        let mut value = int.checked_mul(unit).ok_or_else(err)?;
        if !frac.is_empty() {
            // keep the digits that can still matter at nanosecond scale
            let digits = &frac[..frac.len().min(18)];
            let scale = 10u128.pow(digits.len() as u32);
            value += digits.parse::<u128>().map_err(|_| err())? * unit / scale;
        }
        total = total.checked_add(value).ok_or_else(err)?;
        rest = after;
    }
    let total = i64::try_from(total).map_err(|_| err())?;
    Ok(if neg { -total } else { total })
}

/// Formats like time.Duration.String.
pub fn format(ns: i64) -> String {
    let u = u128::from(ns.unsigned_abs());
    let body = if u < SEC {
        match u {
            0 => return "0s".into(),
            u if u < US => format!("{u}ns"),
            u if u < MS => format!("{}µs", decimal(u, US)),
            u => format!("{}ms", decimal(u, MS)),
        }
    } else {
        let secs = format!("{}s", decimal(u % (60 * SEC), SEC));
        let minutes = u / (60 * SEC);
        match (minutes / 60, minutes % 60) {
            (0, 0) => secs,
            (0, m) => format!("{m}m{secs}"),
            (h, m) => format!("{h}h{m}m{secs}"),
        }
    };
    if ns < 0 { format!("-{body}") } else { body }
}

/// v / unit with the fraction's trailing zeros trimmed.
fn decimal(v: u128, unit: u128) -> String {
    let (int, rem) = (v / unit, v % unit);
    if rem == 0 {
        return int.to_string();
    }
    let width = unit.ilog10() as usize;
    let frac = format!("{rem:0width$}");
    format!("{int}.{}", frac.trim_end_matches('0'))
}

#[cfg(test)]
mod tests {
    use super::*;

    // expected values printed by Go's time package
    #[test]
    fn formats_like_go() {
        for (ns, want) in [
            (0, "0s"),
            (3_600_000_000_000, "1h0m0s"),
            (90_000_000_000, "1m30s"),
            (1_500_000_000, "1.5s"),
            (500_000_000, "500ms"),
            (-60_000_000_000, "-1m0s"),
            (1500, "1.5µs"),
            (10_000_000_000, "10s"),
            (999, "999ns"),
            (93_784_050_000_000, "26h3m4.05s"),
            (1_001_000, "1.001ms"),
        ] {
            assert_eq!(format(ns), want, "{ns}");
        }
    }

    #[test]
    fn parses_like_go() {
        for (s, want) in [
            ("10s", Some(10_000_000_000)),
            ("1h30m", Some(5_400_000_000_000)),
            ("-1m", Some(-60_000_000_000)),
            ("500ms", Some(500_000_000)),
            ("0", Some(0)),
            ("1.5h", Some(5_400_000_000_000)),
            (".5s", Some(500_000_000)),
            ("1us", Some(1000)),
            ("1µs", Some(1000)),
            ("2h45m30.5s", Some(9_930_500_000_000)),
            ("+3s", Some(3_000_000_000)),
            ("1.s", Some(1_000_000_000)),
            ("", None),
            ("10", None),
            ("1x", None),
            ("1e3s", None),
            ("-", None),
        ] {
            assert_eq!(parse(s).ok(), want, "{s:?}");
        }
    }
}

//! Cloud reads, bytes and seeks, per domain and per day.
//!
//! The quota and the breaker count requests, and they stop a scan that
//! has asked too often. This does not. It only records what a resolve
//! actually moved: one range read, the file size that read reported, and
//! whether the read was a seek (a range that did not start at byte 0, or
//! a playback that asked for a fresh link).

use rusqlite::{Result, params};

use super::cache::{Cache, unix_seconds};

/// One day's traffic for one domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayUsage {
    pub domain: String,
    /// `YYYY-MM-DD`, local to the host clock.
    pub day: String,
    pub reads: i64,
    pub bytes: i64,
    pub seeks: i64,
}

pub(super) const USAGE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS cloud_usage (
    domain  TEXT NOT NULL,
    day     TEXT NOT NULL,
    reads   INTEGER NOT NULL,
    bytes   INTEGER NOT NULL,
    seeks   INTEGER NOT NULL,
    PRIMARY KEY (domain, day)
)";

impl Cache {
    /// Adds one cloud read to today's row for `domain`.
    ///
    /// `bytes` is what the read reported, usually the file size from
    /// Content-Range. A negative size is stored as zero. `seek` counts a
    /// read that did not start at the beginning of the file.
    pub fn record_read(
        &self,
        domain: &str,
        bytes: i64,
        seek: bool,
        now: std::time::SystemTime,
    ) -> Result<()> {
        if domain.is_empty() {
            return Ok(());
        }
        let bytes = bytes.max(0);
        self.conn()
            .prepare_cached(
                "INSERT INTO cloud_usage (domain, day, reads, bytes, seeks)
                 VALUES (?, ?, 1, ?, ?)
                 ON CONFLICT(domain, day) DO UPDATE SET
                    reads = reads + 1,
                    bytes = bytes + excluded.bytes,
                    seeks = seeks + excluded.seeks",
            )?
            .execute(params![domain, day_of(now), bytes, i64::from(seek)])?;
        Ok(())
    }

    /// Usage rows for one day, ordered by domain.
    pub fn usage_on(&self, day: &str) -> Result<Vec<DayUsage>> {
        let conn = self.conn();
        let mut stmt = conn.prepare_cached(
            "SELECT domain, day, reads, bytes, seeks FROM cloud_usage
             WHERE day = ? ORDER BY domain",
        )?;
        stmt.query_map([day], |row| {
            Ok(DayUsage {
                domain: row.get(0)?,
                day: row.get(1)?,
                reads: row.get(2)?,
                bytes: row.get(3)?,
                seeks: row.get(4)?,
            })
        })?
        .collect()
    }
}

/// `YYYY-MM-DD` from a unix timestamp shifted into the host's local time.
/// The day boundary is the one the person reading the numbers lives in.
fn day_of(now: std::time::SystemTime) -> String {
    let secs = unix_seconds(now) + local_offset_secs();
    let days = secs.div_euclid(86_400);
    // Civil from days since 1970-01-01, Howard Hinnant's algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let y = y + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Seconds east of UTC, from the C library's local time. Read once: a
/// daemon does not change time zones while it runs.
fn local_offset_secs() -> i64 {
    static OFFSET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFFSET.get_or_init(|| {
        let now = unix_seconds(std::time::SystemTime::now());
        let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
        // localtime_r fills tm_gmtoff. A failure is recorded as UTC.
        let ok = unsafe { libc::localtime_r(&now, local.as_mut_ptr()) };
        if ok.is_null() {
            return 0;
        }
        unsafe { local.assume_init().tm_gmtoff }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::cache::tests::test_cache;
    use std::time::{Duration, UNIX_EPOCH};

    fn at(secs: u64) -> std::time::SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn reads_add_up_and_a_seek_is_counted() {
        let (_dir, cache) = test_cache();
        let now = at(1_700_000_000);
        cache.record_read("cdn.example", 1000, false, now).unwrap();
        cache.record_read("cdn.example", 500, true, now).unwrap();
        cache.record_read("other.example", 7, false, now).unwrap();
        let day = day_of(now);
        let rows = cache.usage_on(&day).unwrap();
        assert_eq!(
            rows,
            [
                DayUsage {
                    domain: "cdn.example".into(),
                    day: day.clone(),
                    reads: 2,
                    bytes: 1500,
                    seeks: 1,
                },
                DayUsage {
                    domain: "other.example".into(),
                    day,
                    reads: 1,
                    bytes: 7,
                    seeks: 0,
                },
            ]
        );
    }

    #[test]
    fn a_later_day_is_its_own_row() {
        let (_dir, cache) = test_cache();
        let first = at(1_700_000_000);
        let next = at(1_700_000_000 + 86_400);
        cache.record_read("cdn.example", 10, false, first).unwrap();
        cache.record_read("cdn.example", 20, true, next).unwrap();
        assert_eq!(cache.usage_on(&day_of(first)).unwrap()[0].reads, 1);
        let later = &cache.usage_on(&day_of(next)).unwrap()[0];
        assert_eq!(later.reads, 1);
        assert_eq!(later.seeks, 1);
        assert_eq!(later.bytes, 20);
    }

    #[test]
    fn an_empty_domain_is_not_a_row() {
        let (_dir, cache) = test_cache();
        let now = at(1_700_000_000);
        cache.record_read("", 10, true, now).unwrap();
        assert!(cache.usage_on(&day_of(now)).unwrap().is_empty());
    }

    #[test]
    fn the_epoch_is_the_first_day() {
        assert_eq!(day_of(UNIX_EPOCH), "1970-01-01");
    }
}

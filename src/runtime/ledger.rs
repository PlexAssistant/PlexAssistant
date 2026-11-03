use std::time::{Duration, SystemTime};

use rusqlite::{OptionalExtension, Result, params};

use super::cache::{Cache, unix_seconds};

const fn days(n: u64) -> Duration {
    Duration::from_secs(n * 24 * 60 * 60)
}

/// Dead-link backoff sequence: first failure retries after a day, then
/// two, four, capped at a week.
const BACKOFF_STEPS: [Duration; 4] = [days(1), days(2), days(4), days(7)];

pub(super) const FAILURES_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS failures (
    target_url TEXT PRIMARY KEY,
    fail_count INTEGER NOT NULL,
    last_fail  INTEGER NOT NULL,
    next_retry INTEGER NOT NULL
)";

impl Cache {
    /// Books a failed target with growing backoff.
    pub fn record_failure(&self, target: &str, now: SystemTime) -> Result<()> {
        // one lock across read and upsert keeps concurrent counts intact
        let conn = self.conn();
        let previous: Option<i64> = conn
            .prepare_cached("SELECT fail_count FROM failures WHERE target_url=?")?
            .query_row([target], |r| r.get(0))
            .optional()?;
        let count = previous.unwrap_or(0) + 1;
        let step = usize::try_from(count - 1)
            .ok()
            .and_then(|i| BACKOFF_STEPS.get(i))
            .unwrap_or(&BACKOFF_STEPS[BACKOFF_STEPS.len() - 1]);
        conn.prepare_cached(
            "INSERT INTO failures (target_url, fail_count, last_fail, next_retry)
             VALUES (?,?,?,?)
             ON CONFLICT(target_url) DO UPDATE SET
                fail_count=excluded.fail_count,
                last_fail=excluded.last_fail,
                next_retry=excluded.next_retry",
        )?
        .execute(params![
            target,
            count,
            unix_seconds(now),
            unix_seconds(now + *step)
        ])?;
        Ok(())
    }

    /// Forgets a target after any success.
    pub fn clear_failure(&self, target: &str) -> Result<()> {
        self.conn()
            .prepare_cached("DELETE FROM failures WHERE target_url=?")?
            .execute([target])?;
        Ok(())
    }

    /// Reports whether the target is still backing off.
    pub fn should_skip(&self, target: &str, now: SystemTime) -> Result<bool> {
        let next: Option<i64> = self
            .conn()
            .prepare_cached("SELECT next_retry FROM failures WHERE target_url=?")?
            .query_row([target], |r| r.get(0))
            .optional()?;
        Ok(next.is_some_and(|next| next > unix_seconds(now)))
    }

    /// Reports booked failures for /state.
    pub fn failure_count(&self) -> Result<i64> {
        self.conn()
            .prepare_cached("SELECT count(*) FROM failures")?
            .query_row([], |r| r.get(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::cache::tests::test_cache;

    const HOUR: Duration = Duration::from_secs(60 * 60);
    const WEEK: Duration = days(7);

    #[test]
    fn backoff() {
        let (_dir, cache) = test_cache();
        let now = SystemTime::now();
        let target = "http://x/file/dead";

        // first failure: retry allowed only after a day
        cache.record_failure(target, now).unwrap();
        assert!(cache.should_skip(target, now + HOUR).unwrap());
        assert!(!cache.should_skip(target, now + 25 * HOUR).unwrap());

        // repeated failures cap the step at a week
        for _ in 0..4 {
            cache.record_failure(target, now).unwrap();
        }
        assert!(cache.should_skip(target, now + WEEK - HOUR).unwrap());
        assert!(!cache.should_skip(target, now + WEEK + HOUR).unwrap());
        assert_eq!(cache.failure_count().unwrap(), 1);

        // success clears the ledger
        cache.clear_failure(target).unwrap();
        assert!(!cache.should_skip(target, now).unwrap());
        assert_eq!(cache.failure_count().unwrap(), 0);
    }

    #[test]
    fn steps_double() {
        let (_dir, cache) = test_cache();
        let now = SystemTime::now();
        let target = "http://x/file/dead";
        for (n, step) in BACKOFF_STEPS.iter().enumerate() {
            cache.record_failure(target, now).unwrap();
            assert!(
                cache.should_skip(target, now + *step - HOUR).unwrap(),
                "failure {n}"
            );
            assert!(
                !cache.should_skip(target, now + *step + HOUR).unwrap(),
                "failure {n}"
            );
        }
    }

    #[test]
    fn unknown_target_is_not_skipped() {
        let (_dir, cache) = test_cache();
        assert!(
            !cache
                .should_skip("never-failed", SystemTime::now())
                .unwrap()
        );
    }
}

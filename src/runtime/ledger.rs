use std::time::{Duration, SystemTime};

use rusqlite::{OptionalExtension, Result, params};

use super::cache::{Cache, unix_seconds};

const fn days(n: u64) -> Duration {
    Duration::from_secs(n * 24 * 60 * 60)
}

/// Dead-link backoff sequence: first failure retries after a day, then
/// two, four, capped at a week.
const BACKOFF_STEPS: [Duration; 4] = [days(1), days(2), days(4), days(7)];

/// Why a target is booked. A temporary failure retries after backoff; a
/// permanent one (a URL that cannot be parsed, a file the upstream says
/// is gone) is skipped until something succeeds and clears the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Temporary,
    Permanent,
}

impl FailureKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Temporary => "temporary",
            Self::Permanent => "permanent",
        }
    }

    fn parse(stored: &str) -> Self {
        match stored {
            "permanent" => Self::Permanent,
            _ => Self::Temporary,
        }
    }
}

pub(super) const FAILURES_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS failures (
    target_url TEXT PRIMARY KEY,
    fail_count INTEGER NOT NULL,
    last_fail  INTEGER NOT NULL,
    next_retry INTEGER NOT NULL,
    kind       TEXT NOT NULL DEFAULT 'temporary'
)";

/// Databases opened before `kind` existed get the column on the next
/// open. Existing rows default to temporary, which is what they were.
const FAILURES_KIND: &str = "
ALTER TABLE failures ADD COLUMN kind TEXT NOT NULL DEFAULT 'temporary'";

pub(super) fn ensure_schema(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(FAILURES_SCHEMA)?;
    let has_kind: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('failures') WHERE name='kind'")?
        .exists([])?;
    if !has_kind {
        conn.execute_batch(FAILURES_KIND)?;
    }
    Ok(())
}

impl Cache {
    /// Books a failed target. Temporary failures grow the backoff;
    /// permanent ones are skipped on every later lookup. A later
    /// permanent booking does not shorten a backoff already in force,
    /// and a temporary booking does not reopen a permanent one.
    pub fn record_failure(&self, target: &str, now: SystemTime) -> Result<()> {
        self.record(target, FailureKind::Temporary, now)
    }

    /// Books a failure that will not clear on its own.
    pub fn record_permanent(&self, target: &str, now: SystemTime) -> Result<()> {
        self.record(target, FailureKind::Permanent, now)
    }

    fn record(&self, target: &str, kind: FailureKind, now: SystemTime) -> Result<()> {
        // one lock across read and upsert keeps concurrent counts intact
        let conn = self.conn();
        let previous: Option<(i64, String)> = conn
            .prepare_cached("SELECT fail_count, kind FROM failures WHERE target_url=?")?
            .query_row([target], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let (count, stored) = match previous {
            Some((count, stored)) => (count + 1, FailureKind::parse(&stored)),
            None => (1, kind),
        };
        // permanent wins: a transient blip must not reopen a dead URL,
        // and a URL that has since been reported gone stays gone
        let kind = if stored == FailureKind::Permanent {
            FailureKind::Permanent
        } else {
            kind
        };
        let step = usize::try_from(count - 1)
            .ok()
            .and_then(|i| BACKOFF_STEPS.get(i))
            .unwrap_or(&BACKOFF_STEPS[BACKOFF_STEPS.len() - 1]);
        conn.prepare_cached(
            "INSERT INTO failures (target_url, fail_count, last_fail, next_retry, kind)
             VALUES (?,?,?,?,?)
             ON CONFLICT(target_url) DO UPDATE SET
                fail_count=excluded.fail_count,
                last_fail=excluded.last_fail,
                next_retry=excluded.next_retry,
                kind=excluded.kind",
        )?
        .execute(params![
            target,
            count,
            unix_seconds(now),
            unix_seconds(now + *step),
            kind.as_str()
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

    /// Reports whether the target should not be resolved again.
    ///
    /// A permanent failure is skipped outright. A temporary one is
    /// skipped only while its backoff has not elapsed.
    pub fn should_skip(&self, target: &str, now: SystemTime) -> Result<bool> {
        let row: Option<(i64, String)> = self
            .conn()
            .prepare_cached("SELECT next_retry, kind FROM failures WHERE target_url=?")?
            .query_row([target], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        Ok(row.is_some_and(|(next, kind)| {
            FailureKind::parse(&kind) == FailureKind::Permanent || next > unix_seconds(now)
        }))
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
    fn permanent_is_skipped_until_cleared() {
        let (_dir, cache) = test_cache();
        let now = SystemTime::now();
        let target = "http://x/file/gone";

        cache.record_permanent(target, now).unwrap();
        // no backoff to wait out: a later lookup is still a skip
        assert!(cache.should_skip(target, now + 8 * WEEK).unwrap());

        // a later temporary booking must not reopen it
        cache.record_failure(target, now).unwrap();
        assert!(cache.should_skip(target, now + 8 * WEEK).unwrap());

        cache.clear_failure(target).unwrap();
        assert!(!cache.should_skip(target, now).unwrap());
    }

    #[test]
    fn permanent_does_not_shorten_an_open_backoff() {
        let (_dir, cache) = test_cache();
        let now = SystemTime::now();
        let target = "http://x/file/flaky";

        cache.record_failure(target, now).unwrap();
        cache.record_permanent(target, now).unwrap();
        // still inside the first day's backoff, and now permanent
        assert!(cache.should_skip(target, now + HOUR).unwrap());
        assert!(cache.should_skip(target, now + 8 * WEEK).unwrap());
    }

    /// A database written before `kind` existed still opens, and its
    /// rows keep the old backoff behavior.
    #[test]
    fn old_ledger_gains_kind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE failures (
                target_url TEXT PRIMARY KEY,
                fail_count INTEGER NOT NULL,
                last_fail  INTEGER NOT NULL,
                next_retry INTEGER NOT NULL
            )",
        )
        .unwrap();
        let later = unix_seconds(SystemTime::now() + WEEK);
        conn.execute(
            "INSERT INTO failures (target_url, fail_count, last_fail, next_retry) VALUES (?,?,?,?)",
            rusqlite::params!["http://x/file/old", 1, 0, later],
        )
        .unwrap();
        drop(conn);

        let cache = Cache::open(&path).unwrap();
        assert!(cache.should_skip("http://x/file/old", SystemTime::now()).unwrap());
        // the migrated row is temporary: past its backoff it is tried again
        assert!(!cache
            .should_skip("http://x/file/old", SystemTime::now() + 2 * WEEK)
            .unwrap());
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

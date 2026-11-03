//! Why one part was not written.
//!
//! The link ledger books a URL. A detail page asks about a part: it was
//! probed, it failed, and the library row is still empty. The two are not
//! the same row. A part that cannot succeed is left alone; one that might
//! is tried again, and a later success clears the row.

use std::time::SystemTime;

use rusqlite::{OptionalExtension, Result, params};

use super::cache::{Cache, unix_seconds};

/// How a part failed, and whether another attempt can help.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartFailure {
    /// The strm could not be read, or the upstream says the file is gone.
    Permanent,
    /// A refusal, a stall, or a probe that failed for a reason that can
    /// clear. Tried again next time.
    Temporary,
    /// A person, or a previous decision, said not to try this part again.
    Ignored,
}

impl PartFailure {
    fn as_str(self) -> &'static str {
        match self {
            Self::Permanent => "permanent",
            Self::Temporary => "temporary",
            Self::Ignored => "ignored",
        }
    }

    fn parse(stored: &str) -> Self {
        match stored {
            "permanent" => Self::Permanent,
            "ignored" => Self::Ignored,
            _ => Self::Temporary,
        }
    }
}

/// One booked part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartRecord {
    pub part_id: i64,
    pub attempts: i64,
    pub kind: PartFailure,
}

pub(super) const PARTS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS part_failures (
    part_id    INTEGER PRIMARY KEY,
    attempts   INTEGER NOT NULL,
    kind       TEXT NOT NULL,
    last_at    INTEGER NOT NULL
)";

impl Cache {
    /// The booked failure for a part, if one has been recorded.
    pub fn part_failure(&self, part_id: i64) -> Result<Option<PartRecord>> {
        self.conn()
            .prepare_cached("SELECT part_id, attempts, kind FROM part_failures WHERE part_id=?")?
            .query_row([part_id], |row| {
                let kind: String = row.get(2)?;
                Ok(PartRecord {
                    part_id: row.get(0)?,
                    attempts: row.get(1)?,
                    kind: PartFailure::parse(&kind),
                })
            })
            .optional()
    }

    /// Books another failed attempt. An ignored part stays ignored, and a
    /// permanent one is not reopened by a later temporary failure.
    pub fn record_part_failure(
        &self,
        part_id: i64,
        kind: PartFailure,
        now: SystemTime,
    ) -> Result<()> {
        let conn = self.conn();
        let previous: Option<String> = conn
            .prepare_cached("SELECT kind FROM part_failures WHERE part_id=?")?
            .query_row([part_id], |row| row.get(0))
            .optional()?;
        let kind = match previous.as_deref().map(PartFailure::parse) {
            Some(PartFailure::Ignored) => PartFailure::Ignored,
            Some(PartFailure::Permanent) if kind == PartFailure::Temporary => {
                PartFailure::Permanent
            }
            _ => kind,
        };
        conn.prepare_cached(
            "INSERT INTO part_failures (part_id, attempts, kind, last_at)
             VALUES (?,1,?,?)
             ON CONFLICT(part_id) DO UPDATE SET
                attempts=attempts+1,
                kind=excluded.kind,
                last_at=excluded.last_at",
        )?
        .execute(params![part_id, kind.as_str(), unix_seconds(now)])?;
        Ok(())
    }

    /// Forgets a part after it was written.
    pub fn clear_part_failure(&self, part_id: i64) -> Result<()> {
        self.conn()
            .prepare_cached("DELETE FROM part_failures WHERE part_id=?")?
            .execute([part_id])?;
        Ok(())
    }

    /// Whether this part should not be tried again.
    ///
    /// Permanent and ignored rows are skipped. A temporary one is tried
    /// again: the link ledger already backs off the URL.
    pub fn part_skipped(&self, part_id: i64) -> Result<bool> {
        Ok(self
            .part_failure(part_id)?
            .is_some_and(|row| matches!(row.kind, PartFailure::Permanent | PartFailure::Ignored)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::cache::tests::test_cache;

    #[test]
    fn attempts_grow_and_a_success_clears_them() {
        let (_dir, cache) = test_cache();
        let now = SystemTime::now();
        assert!(!cache.part_skipped(7).unwrap());
        cache
            .record_part_failure(7, PartFailure::Temporary, now)
            .unwrap();
        cache
            .record_part_failure(7, PartFailure::Temporary, now)
            .unwrap();
        let row = cache.part_failure(7).unwrap().unwrap();
        assert_eq!(row.attempts, 2);
        assert_eq!(row.kind, PartFailure::Temporary);
        assert!(!cache.part_skipped(7).unwrap());

        cache.clear_part_failure(7).unwrap();
        assert!(cache.part_failure(7).unwrap().is_none());
    }

    #[test]
    fn permanent_is_skipped_and_not_reopened() {
        let (_dir, cache) = test_cache();
        let now = SystemTime::now();
        cache
            .record_part_failure(7, PartFailure::Permanent, now)
            .unwrap();
        cache
            .record_part_failure(7, PartFailure::Temporary, now)
            .unwrap();
        assert!(cache.part_skipped(7).unwrap());
        assert_eq!(
            cache.part_failure(7).unwrap().unwrap().kind,
            PartFailure::Permanent
        );
        cache.clear_part_failure(7).unwrap();
        assert!(!cache.part_skipped(7).unwrap());
    }

    #[test]
    fn ignored_stays_ignored() {
        let (_dir, cache) = test_cache();
        let now = SystemTime::now();
        cache
            .record_part_failure(7, PartFailure::Ignored, now)
            .unwrap();
        cache
            .record_part_failure(7, PartFailure::Temporary, now)
            .unwrap();
        assert!(cache.part_skipped(7).unwrap());
        assert_eq!(cache.part_failure(7).unwrap().unwrap().attempts, 2);
    }
}

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{fs, io};

use rusqlite::{Connection, OptionalExtension, Result, params};

use super::ledger::FAILURES_SCHEMA;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS links (
    target_url  TEXT PRIMARY KEY,
    direct_url  TEXT NOT NULL,
    ua          TEXT,
    expires_at  INTEGER,
    cached_at   INTEGER NOT NULL,
    last_used   INTEGER NOT NULL,
    hits        INTEGER NOT NULL DEFAULT 0
)";

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// One cached direct link.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    pub target: String,
    pub direct: String,
    pub ua: String,
    /// unix seconds, 0 = unknown (informational)
    pub expires_at: i64,
    pub cached_at: i64,
    pub last_used: i64,
    pub hits: i64,
}

/// Cache totals for /state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub entries: i64,
    pub hits: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("create {}: {source}", dir.display())]
    CreateDir { dir: PathBuf, source: io::Error },
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

/// The persistent link store. The connection sits behind a mutex so the
/// daemon's concurrent handlers can share one cache.
pub struct Cache {
    conn: Mutex<Connection>,
}

impl Cache {
    /// Opens (creating if needed) the cache database at path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, OpenError> {
        let path = path.as_ref();
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir).map_err(|source| OpenError::CreateDir {
                dir: dir.to_owned(),
                source,
            })?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.execute_batch(SCHEMA)?;
        conn.execute_batch(FAILURES_SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// A poisoned lock only means another handler panicked mid-call;
    /// SQLite itself stays consistent, so keep serving.
    pub(super) fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Returns the cached entry, bumping its use counters, or None.
    pub fn get(&self, target: &str) -> Result<Option<Entry>> {
        let conn = self.conn();
        let entry = conn
            .prepare_cached(
                "SELECT target_url, direct_url, IFNULL(ua,''), IFNULL(expires_at,0),
                        cached_at, last_used, hits FROM links WHERE target_url=?",
            )?
            .query_row([target], |r| {
                Ok(Entry {
                    target: r.get(0)?,
                    direct: r.get(1)?,
                    ua: r.get(2)?,
                    expires_at: r.get(3)?,
                    cached_at: r.get(4)?,
                    last_used: r.get(5)?,
                    hits: r.get(6)?,
                })
            })
            .optional()?;
        let Some(mut entry) = entry else {
            return Ok(None);
        };
        let ts = now();
        conn.prepare_cached("UPDATE links SET hits=hits+1, last_used=? WHERE target_url=?")?
            .execute(params![ts, target])?;
        entry.hits += 1;
        entry.last_used = ts;
        Ok(Some(entry))
    }

    /// Upserts an entry. cached_at is kept from the first insert and the
    /// hit counter survives a link replacement.
    pub fn put(&self, entry: &Entry) -> Result<()> {
        let ts = now();
        let cached_at = if entry.cached_at == 0 {
            ts
        } else {
            entry.cached_at
        };
        self.conn()
            .prepare_cached(
                "INSERT INTO links (target_url, direct_url, ua, expires_at, cached_at, last_used, hits)
                 VALUES (?,?,?,?,?,?,0)
                 ON CONFLICT(target_url) DO UPDATE SET
                    direct_url=excluded.direct_url, ua=excluded.ua,
                    expires_at=excluded.expires_at, last_used=excluded.last_used",
            )?
            .execute(params![
                entry.target,
                entry.direct,
                entry.ua,
                entry.expires_at,
                cached_at,
                ts
            ])?;
        Ok(())
    }

    /// Reports cache totals.
    pub fn stats(&self) -> Result<Stats> {
        self.conn()
            .prepare_cached("SELECT count(*), IFNULL(sum(hits),0) FROM links")?
            .query_row([], |r| {
                Ok(Stats {
                    entries: r.get(0)?,
                    hits: r.get(1)?,
                })
            })
    }
}

fn now() -> i64 {
    unix_seconds(SystemTime::now())
}

pub(super) fn unix_seconds(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(in crate::runtime) fn test_cache() -> (tempfile::TempDir, Cache) {
        let dir = tempfile::tempdir().unwrap();
        // a missing parent directory is created on open
        let cache = Cache::open(dir.path().join("assistant/cache.db")).unwrap();
        (dir, cache)
    }

    #[test]
    fn roundtrip() {
        let (_dir, cache) = test_cache();
        let target = "http://x/file/ep1";

        assert_eq!(cache.get(target).unwrap(), None);
        cache
            .put(&Entry {
                target: target.into(),
                direct: "http://x/dl/ep1?t=123".into(),
                expires_at: 1799999999,
                ..Default::default()
            })
            .unwrap();

        let e = cache.get(target).unwrap().unwrap();
        assert_eq!(e.direct, "http://x/dl/ep1?t=123");
        assert_eq!(e.expires_at, 1799999999);
        assert_eq!(e.hits, 1);
        let first_cached = e.cached_at;

        // upsert replaces the link but keeps cached_at and hits
        cache
            .put(&Entry {
                target: target.into(),
                direct: "http://x/dl/ep1?t=456".into(),
                ..Default::default()
            })
            .unwrap();
        let e = cache.get(target).unwrap().unwrap();
        assert_eq!(e.direct, "http://x/dl/ep1?t=456");
        assert_eq!(e.expires_at, 0);
        assert_eq!(e.hits, 2);
        assert_eq!(e.cached_at, first_cached);
    }

    #[test]
    fn stats() {
        let (_dir, cache) = test_cache();
        assert_eq!(cache.stats().unwrap(), Stats::default());
        for target in ["a", "b"] {
            cache
                .put(&Entry {
                    target: target.into(),
                    direct: "d".into(),
                    ..Default::default()
                })
                .unwrap();
        }
        cache.get("a").unwrap();
        cache.get("a").unwrap();
        assert_eq!(
            cache.stats().unwrap(),
            Stats {
                entries: 2,
                hits: 2
            }
        );
    }

    #[test]
    fn reopen_keeps_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.db");
        let entry = Entry {
            target: "t".into(),
            direct: "d".into(),
            ..Default::default()
        };
        Cache::open(&path).unwrap().put(&entry).unwrap();
        let reopened = Cache::open(&path).unwrap();
        assert_eq!(reopened.get("t").unwrap().unwrap().direct, "d");
    }
}

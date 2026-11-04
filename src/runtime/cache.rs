use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{fs, io};

use rusqlite::{Connection, OptionalExtension, Result, params};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS links (
    target_url  TEXT PRIMARY KEY,
    direct_url  TEXT NOT NULL,
    ua          TEXT,
    expires_at  INTEGER,
    cached_at   INTEGER NOT NULL,
    last_used   INTEGER NOT NULL,
    hits        INTEGER NOT NULL DEFAULT 0,
    size        INTEGER NOT NULL DEFAULT 0
)";

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The cache inside the container.
pub const DEFAULT_CACHE: &str = "/config/assistant/cache.db";

/// `ASSISTANT_CACHE` when set and not empty, else [`DEFAULT_CACHE`]. Lets
/// a scan outside the container point at its own cache.
pub fn cache_path() -> PathBuf {
    std::env::var_os("ASSISTANT_CACHE")
        .filter(|v| !v.is_empty())
        .map_or_else(|| PathBuf::from(DEFAULT_CACHE), PathBuf::from)
}

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
    /// bytes the resolve learned, 0 when unknown
    pub size: i64,
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
        ensure_link_size(&conn)?;
        super::ledger::ensure_schema(&conn)?;
        conn.execute_batch(super::markers::MARKERS_SCHEMA)?;
        conn.execute_batch(super::identity::IDENTITY_SCHEMA)?;
        conn.execute_batch(super::parts::PARTS_SCHEMA)?;
        conn.execute_batch(super::tracks::TRACKS_SCHEMA)?;
        conn.execute_batch(super::usage::USAGE_SCHEMA)?;
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
                        cached_at, last_used, hits, IFNULL(size,0) FROM links WHERE target_url=?",
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
                    size: r.get(7)?,
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
        put_link_on(&self.conn(), entry, cached_at, ts)
    }

    /// Books a successful resolve: drops the failure row, counts the read,
    /// and stores the link. The three writes commit together, so a crash
    /// cannot leave a counted read without its link, or a link whose failure
    /// row is still in force.
    pub fn store_resolved(
        &self,
        entry: &Entry,
        domain: &str,
        bytes: i64,
        seek: bool,
        now_at: SystemTime,
    ) -> Result<()> {
        let ts = unix_seconds(now_at);
        let cached_at = if entry.cached_at == 0 {
            ts
        } else {
            entry.cached_at
        };
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        super::ledger::clear_failure_on(&tx, &entry.target)?;
        super::usage::record_read_on(&tx, domain, bytes, seek, now_at)?;
        put_link_on(&tx, entry, cached_at, ts)?;
        tx.commit()?;
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

fn put_link_on(conn: &Connection, entry: &Entry, cached_at: i64, ts: i64) -> Result<()> {
    conn.prepare_cached(
        "INSERT INTO links (target_url, direct_url, ua, expires_at, cached_at, last_used, hits, size)
         VALUES (?,?,?,?,?,?,0,?)
         ON CONFLICT(target_url) DO UPDATE SET
            direct_url=excluded.direct_url, ua=excluded.ua,
            expires_at=excluded.expires_at, last_used=excluded.last_used,
            size=excluded.size",
    )?
    .execute(params![
        entry.target,
        entry.direct,
        entry.ua,
        entry.expires_at,
        cached_at,
        ts,
        entry.size,
    ])?;
    Ok(())
}

/// Adds `size` to a cache written before that column existed.
fn ensure_link_size(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(links)")?;
    let names: Vec<String> = stmt
        .query_map([], |row| row.get(1))?
        .collect::<Result<_, _>>()?;
    if !names.iter().any(|name| name == "size") {
        conn.execute(
            "ALTER TABLE links ADD COLUMN size INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    Ok(())
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
    fn a_resolved_link_clears_its_failure_and_counts_the_read() {
        let (_dir, cache) = test_cache();
        let target = "http://cdn.example/file/ep1";
        cache
            .record_failure(target, SystemTime::UNIX_EPOCH)
            .unwrap();
        cache
            .store_resolved(
                &Entry {
                    target: target.into(),
                    direct: "http://cdn.example/dl/ep1?t=1".into(),
                    size: 40,
                    ..Default::default()
                },
                "cdn.example",
                40,
                false,
                SystemTime::UNIX_EPOCH,
            )
            .unwrap();
        assert!(!cache.should_skip(target, SystemTime::UNIX_EPOCH).unwrap());
        assert_eq!(cache.get(target).unwrap().unwrap().size, 40);
        let usage = cache.usage_on("1970-01-01").unwrap();
        assert_eq!(usage.len(), 1);
        assert_eq!((usage[0].reads, usage[0].bytes), (1, 40));
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

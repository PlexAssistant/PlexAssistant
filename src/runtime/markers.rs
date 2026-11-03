//! What detection already decided about an item, so a later scan can
//! tell "nothing changed" from "the file changed" without re-running.
//!
//! The row lives in the assistant's own cache, not in Plex's library.
//! Plex holds the marker a client sees; this holds why it was written
//! and whether detection may replace it.

use std::time::SystemTime;

use rusqlite::{OptionalExtension, Result, params};

use super::cache::{Cache, unix_seconds};

/// Who wrote the marker, and whether detection may replace it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerOrigin {
    /// Detection wrote it. A changed digest re-runs detection.
    Auto,
    /// A person set it. Detection never replaces it.
    Manual,
    /// Detection is switched off for this item and marker type.
    Stopped,
}

impl MarkerOrigin {
    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual => "manual",
            Self::Stopped => "stopped",
        }
    }

    fn parse(stored: &str) -> Self {
        match stored {
            "manual" => Self::Manual,
            "stopped" => Self::Stopped,
            _ => Self::Auto,
        }
    }
}

/// One booked marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkerRecord {
    pub item_id: i64,
    pub marker_type: i64,
    pub start_ms: i64,
    pub end_ms: i64,
    /// Hash of the content key the marker was detected against.
    pub digest: String,
    pub origin: MarkerOrigin,
}

pub(super) const MARKERS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS marker_ledger (
    item_id     INTEGER NOT NULL,
    marker_type INTEGER NOT NULL,
    start_ms    INTEGER NOT NULL,
    end_ms      INTEGER NOT NULL,
    digest      TEXT NOT NULL,
    origin      TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (item_id, marker_type)
)";

/// What a scan should do before it spends a detection run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerDecision {
    /// No row, or the content changed: detect and write.
    Detect,
    /// The row stands and the content is unchanged: do nothing.
    Skip,
}

impl Cache {
    /// The booked marker, if detection has run for this item before.
    pub fn marker(&self, item_id: i64, marker_type: i64) -> Result<Option<MarkerRecord>> {
        self.conn()
            .prepare_cached(
                "SELECT item_id, marker_type, start_ms, end_ms, digest, origin
                   FROM marker_ledger WHERE item_id=? AND marker_type=?",
            )?
            .query_row(params![item_id, marker_type], read_marker)
            .optional()
    }

    /// Every booked marker, oldest item first.
    pub fn markers(&self) -> Result<Vec<MarkerRecord>> {
        let conn = self.conn();
        let mut stmt = conn.prepare_cached(
            "SELECT item_id, marker_type, start_ms, end_ms, digest, origin
               FROM marker_ledger ORDER BY item_id, marker_type",
        )?;
        let rows = stmt.query_map([], read_marker)?;
        rows.collect()
    }

    /// Books a marker detection wrote. A manual or stopped row is left
    /// alone: detection does not get to take it back.
    pub fn record_marker(
        &self,
        item_id: i64,
        marker_type: i64,
        start_ms: i64,
        end_ms: i64,
        digest: &str,
        now: SystemTime,
    ) -> Result<bool> {
        self.upsert(
            item_id,
            marker_type,
            start_ms,
            end_ms,
            digest,
            MarkerOrigin::Auto,
            false,
            now,
        )
    }

    /// Books a marker a person set. Replaces whatever detection wrote.
    pub fn override_marker(
        &self,
        item_id: i64,
        marker_type: i64,
        start_ms: i64,
        end_ms: i64,
        digest: &str,
        now: SystemTime,
    ) -> Result<bool> {
        self.upsert(
            item_id,
            marker_type,
            start_ms,
            end_ms,
            digest,
            MarkerOrigin::Manual,
            true,
            now,
        )
    }

    /// Stops detection for one item and marker type. The times are kept,
    /// so a later retry knows what was there.
    pub fn stop_marker(&self, item_id: i64, marker_type: i64, now: SystemTime) -> Result<bool> {
        let Some(existing) = self.marker(item_id, marker_type)? else {
            return Ok(false);
        };
        self.upsert(
            item_id,
            marker_type,
            existing.start_ms,
            existing.end_ms,
            &existing.digest,
            MarkerOrigin::Stopped,
            true,
            now,
        )
    }

    /// Forgets a row so the next scan detects again.
    pub fn retry_marker(&self, item_id: i64, marker_type: i64) -> Result<bool> {
        let n = self
            .conn()
            .prepare_cached("DELETE FROM marker_ledger WHERE item_id=? AND marker_type=?")?
            .execute(params![item_id, marker_type])?;
        Ok(n > 0)
    }

    /// Whether the next scan should detect.
    ///
    /// A manual or stopped row is never re-detected. An automatic one is
    /// re-detected only when the content digest changed.
    pub fn marker_decision(
        &self,
        item_id: i64,
        marker_type: i64,
        digest: &str,
    ) -> Result<MarkerDecision> {
        let Some(row) = self.marker(item_id, marker_type)? else {
            return Ok(MarkerDecision::Detect);
        };
        if row.origin != MarkerOrigin::Auto || row.digest == digest {
            return Ok(MarkerDecision::Skip);
        }
        Ok(MarkerDecision::Detect)
    }

    #[allow(clippy::too_many_arguments)]
    fn upsert(
        &self,
        item_id: i64,
        marker_type: i64,
        start_ms: i64,
        end_ms: i64,
        digest: &str,
        origin: MarkerOrigin,
        replace_protected: bool,
        now: SystemTime,
    ) -> Result<bool> {
        let conn = self.conn();
        let existing: Option<String> = conn
            .prepare_cached("SELECT origin FROM marker_ledger WHERE item_id=? AND marker_type=?")?
            .query_row(params![item_id, marker_type], |r| r.get(0))
            .optional()?;
        if !replace_protected
            && existing
                .as_deref()
                .is_some_and(|stored| MarkerOrigin::parse(stored) != MarkerOrigin::Auto)
        {
            return Ok(false);
        }
        let ts = unix_seconds(now);
        let created = if existing.is_some() { 0 } else { ts };
        conn.prepare_cached(
            "INSERT INTO marker_ledger
                 (item_id, marker_type, start_ms, end_ms, digest, origin, created_at, updated_at)
             VALUES (?,?,?,?,?,?,?,?)
             ON CONFLICT(item_id, marker_type) DO UPDATE SET
                start_ms=excluded.start_ms,
                end_ms=excluded.end_ms,
                digest=excluded.digest,
                origin=excluded.origin,
                updated_at=excluded.updated_at",
        )?
        .execute(params![
            item_id,
            marker_type,
            start_ms,
            end_ms,
            digest,
            origin.as_str(),
            created,
            ts
        ])?;
        Ok(true)
    }
}

fn read_marker(row: &rusqlite::Row<'_>) -> Result<MarkerRecord> {
    let origin: String = row.get(5)?;
    Ok(MarkerRecord {
        item_id: row.get(0)?,
        marker_type: row.get(1)?,
        start_ms: row.get(2)?,
        end_ms: row.get(3)?,
        digest: row.get(4)?,
        origin: MarkerOrigin::parse(&origin),
    })
}

/// The fingerprint detection compares against.
///
/// This is the content key, not the strm target. A resigned link or a
/// re-added item keeps the same key, so detection is not repeated. The
/// query string of a target is not an identity: drives put signatures
/// there.
pub fn marker_digest(content_key: &str) -> String {
    use sha2::{Digest, Sha256};
    hex(&Sha256::digest(content_key.as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0xf) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::cache::tests::test_cache;

    const INTRO: i64 = 1;
    const CREDITS: i64 = 5;

    #[test]
    fn unchanged_content_is_skipped() {
        let (_dir, cache) = test_cache();
        let now = SystemTime::now();
        let digest = marker_digest("http://x/file/ep1");

        assert_eq!(
            cache.marker_decision(42, CREDITS, &digest).unwrap(),
            MarkerDecision::Detect
        );
        assert!(
            cache
                .record_marker(42, CREDITS, 30_000, 58_000, &digest, now)
                .unwrap()
        );
        assert_eq!(
            cache.marker_decision(42, CREDITS, &digest).unwrap(),
            MarkerDecision::Skip
        );

        // the same content key, however the file was reached, still skips
        assert_eq!(
            cache.marker_decision(42, CREDITS, &digest).unwrap(),
            MarkerDecision::Skip
        );
    }

    #[test]
    fn changed_content_is_redetected() {
        let (_dir, cache) = test_cache();
        let now = SystemTime::now();
        cache
            .record_marker(
                42,
                CREDITS,
                30_000,
                58_000,
                &marker_digest("http://x/a"),
                now,
            )
            .unwrap();
        assert_eq!(
            cache
                .marker_decision(42, CREDITS, &marker_digest("http://x/b"))
                .unwrap(),
            MarkerDecision::Detect
        );
    }

    #[test]
    fn manual_and_stopped_are_kept() {
        let (_dir, cache) = test_cache();
        let now = SystemTime::now();
        let digest = marker_digest("http://x/file/ep1");

        cache
            .override_marker(42, INTRO, 1_000, 20_000, &digest, now)
            .unwrap();
        // detection must neither replace it nor be asked to re-run
        assert!(
            !cache
                .record_marker(42, INTRO, 5_000, 25_000, &digest, now)
                .unwrap()
        );
        assert_eq!(
            cache
                .marker_decision(42, INTRO, &marker_digest("http://x/other"))
                .unwrap(),
            MarkerDecision::Skip
        );
        assert_eq!(cache.marker(42, INTRO).unwrap().unwrap().start_ms, 1_000);

        assert!(cache.stop_marker(42, INTRO, now).unwrap());
        assert_eq!(
            cache.marker(42, INTRO).unwrap().unwrap().origin,
            MarkerOrigin::Stopped
        );
        assert_eq!(
            cache.marker_decision(42, INTRO, &digest).unwrap(),
            MarkerDecision::Skip
        );

        // stopping an item detection never ran for does nothing
        assert!(!cache.stop_marker(99, CREDITS, now).unwrap());
    }

    #[test]
    fn retry_forgets_the_row() {
        let (_dir, cache) = test_cache();
        let now = SystemTime::now();
        let digest = marker_digest("http://x/file/ep1");
        cache.stop_marker(7, CREDITS, now).unwrap();
        cache.record_marker(7, CREDITS, 1, 2, &digest, now).unwrap();
        assert!(cache.retry_marker(7, CREDITS).unwrap());
        assert_eq!(cache.marker(7, CREDITS).unwrap(), None);
        assert!(!cache.retry_marker(7, CREDITS).unwrap());
        assert_eq!(
            cache.marker_decision(7, CREDITS, &digest).unwrap(),
            MarkerDecision::Detect
        );
    }

    #[test]
    fn digest_follows_the_content_key() {
        assert_ne!(marker_digest("d=1\ns=2\n"), marker_digest("d=9\ns=2\n"));
        assert_eq!(marker_digest("d=1\ns=2\n"), marker_digest("d=1\ns=2\n"));
    }
}

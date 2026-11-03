//! What a probed file is, apart from the URL that pointed at it.
//!
//! A strm target changes when the path changes, the file is re-added, or
//! the drive resigns the link. The probe does not: duration, size and the
//! stream layout stay put. Remembering that lets a later scan copy the
//! rows it already wrote instead of probing again.

use rusqlite::{OptionalExtension, Result, params};

use super::cache::Cache;
use crate::scanner::{ProbeResult, Stream};

/// One stream as it contributes to the identity. Order is the probe's.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Track {
    index: i64,
    codec_type: String,
    codec_name: String,
    width: i64,
    height: i64,
    channels: i64,
    sample_rate: String,
    language: String,
}

/// The probed facts that stay the same when only the link changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub duration_ms: i64,
    pub size: i64,
    pub format_name: String,
    tracks: Vec<Track>,
}

impl Identity {
    /// Builds an identity from a probe. A probe with no duration and no
    /// size cannot tell two files apart, so it is not an identity.
    pub fn from_probe(probe: &ProbeResult) -> Option<Self> {
        let duration_ms = crate::scanner::duration_ms(&probe.format.duration);
        let size = probe.format.size.trim().parse::<i64>().unwrap_or(0);
        if duration_ms <= 0 && size <= 0 {
            return None;
        }
        let tracks = probe
            .streams
            .iter()
            .filter(|s| !s.codec_type.is_empty())
            .map(track_of)
            .collect();
        Some(Self {
            duration_ms,
            size,
            format_name: probe.format.format_name.clone(),
            tracks,
        })
    }

    /// Stable text the ledger stores and compares. Field order is fixed,
    /// so the same probe always produces the same line.
    pub fn key(&self) -> String {
        let mut out = format!(
            "d={}\ns={}\nf={}\n",
            self.duration_ms, self.size, self.format_name
        );
        for track in &self.tracks {
            out.push_str(&format!(
                "t={},{},{},{}x{},{},{},{}\n",
                track.index,
                track.codec_type,
                track.codec_name,
                track.width,
                track.height,
                track.channels,
                track.sample_rate,
                track.language,
            ));
        }
        out
    }
}

fn track_of(stream: &Stream) -> Track {
    Track {
        index: stream.index,
        codec_type: stream.codec_type.clone(),
        codec_name: stream.codec_name.clone(),
        width: stream.width,
        height: stream.height,
        channels: stream.channels,
        sample_rate: stream.sample_rate.clone(),
        language: stream.tags.language.clone(),
    }
}

pub(super) const IDENTITY_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS content (
    content_key TEXT PRIMARY KEY,
    duration_ms INTEGER NOT NULL,
    size        INTEGER NOT NULL,
    spec_json   TEXT NOT NULL,
    created_at  INTEGER NOT NULL
)";

/// A stored analysis, ready to be written onto another part.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredSpec {
    pub content_key: String,
    pub spec: crate::plexdb::WriteSpec,
}

impl Cache {
    /// The analysis remembered for this content, if a probe has landed.
    pub fn content(&self, key: &str) -> Result<Option<StoredSpec>> {
        let json: Option<String> = self
            .conn()
            .prepare_cached("SELECT spec_json FROM content WHERE content_key=?")?
            .query_row([key], |row| row.get(0))
            .optional()?;
        let Some(json) = json else {
            return Ok(None);
        };
        let spec = serde_json::from_str(&json).unwrap_or_default();
        Ok(Some(StoredSpec {
            content_key: key.to_owned(),
            spec,
        }))
    }

    /// The analysis whose stored duration and size match a library part.
    ///
    /// Duration and size together are specific enough that two different
    /// files colliding is not worth a probe. A part that matches nothing
    /// is probed as usual.
    pub fn content_for_media(&self, duration_ms: i64, size: i64) -> Result<Option<StoredSpec>> {
        let json: Option<(String, String)> = self
            .conn()
            .prepare_cached(
                "SELECT content_key, spec_json FROM content
                  WHERE duration_ms=? AND size=?
                  LIMIT 1",
            )?
            .query_row(params![duration_ms, size], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()?;
        let Some((content_key, json)) = json else {
            return Ok(None);
        };
        let spec = serde_json::from_str(&json).unwrap_or_default();
        Ok(Some(StoredSpec { content_key, spec }))
    }

    /// Remembers an analysis. A later probe of the same content replaces
    /// the row, so a corrected probe wins over the first one.
    pub fn remember_content(
        &self,
        key: &str,
        spec: &crate::plexdb::WriteSpec,
        now: std::time::SystemTime,
    ) -> Result<()> {
        let json = serde_json::to_string(spec).unwrap_or_default();
        let ts = super::cache::unix_seconds(now);
        self.conn()
            .prepare_cached(
                "INSERT INTO content (content_key, duration_ms, size, spec_json, created_at)
                 VALUES (?,?,?,?,?)
                 ON CONFLICT(content_key) DO UPDATE SET
                    duration_ms=excluded.duration_ms,
                    size=excluded.size,
                    spec_json=excluded.spec_json",
            )?
            .execute(params![key, spec.duration_ms, spec.size, json, ts])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::cache::tests::test_cache;
    use crate::scanner::{ProbeResult, build_spec};
    use std::time::SystemTime;

    const SAMPLE: &str = r#"{
      "streams": [
        {"index": 0, "codec_name": "h264", "codec_type": "video", "width": 1280, "height": 720},
        {"index": 1, "codec_name": "aac", "codec_type": "audio", "channels": 2, "sample_rate": "48000", "tags": {"language": "und"}}
      ],
      "format": {"format_name": "matroska,webm", "duration": "20.016000", "size": "3757806"}
    }"#;

    fn sample() -> ProbeResult {
        serde_json::from_str(SAMPLE).unwrap()
    }

    #[test]
    fn the_same_probe_is_the_same_content() {
        let probe = sample();
        let a = Identity::from_probe(&probe).unwrap();
        let b = Identity::from_probe(&probe).unwrap();
        assert_eq!(a.key(), b.key());
    }

    #[test]
    fn a_different_duration_is_different_content() {
        let mut other = sample();
        other.format.duration = "99.000000".into();
        let a = Identity::from_probe(&sample()).unwrap();
        let b = Identity::from_probe(&other).unwrap();
        assert_ne!(a.key(), b.key());
    }

    #[test]
    fn a_probe_with_nothing_stable_is_not_content() {
        let empty = ProbeResult::default();
        assert!(Identity::from_probe(&empty).is_none());
    }

    #[test]
    fn remembered_content_roundtrips() {
        let (_dir, cache) = test_cache();
        let probe = sample();
        let identity = Identity::from_probe(&probe).unwrap();
        let part = crate::plexdb::Part {
            id: 7,
            media_item_id: 3,
            file: "/a.strm".into(),
            size: 0,
            duration: 0,
        };
        let spec = build_spec(
            &part,
            &probe,
            &crate::linkres::Link {
                size: 3757806,
                ..Default::default()
            },
        );
        assert!(cache.content(&identity.key()).unwrap().is_none());
        cache
            .remember_content(&identity.key(), &spec, SystemTime::now())
            .unwrap();
        let stored = cache.content(&identity.key()).unwrap().unwrap();
        assert_eq!(stored.spec.duration_ms, spec.duration_ms);
        assert_eq!(stored.spec.video_codec, spec.video_codec);
        assert_eq!(stored.spec.streams.len(), spec.streams.len());
        // the stored row is not tied to the part it was first written for
        assert_eq!(stored.spec.part_id, 7);
    }
}

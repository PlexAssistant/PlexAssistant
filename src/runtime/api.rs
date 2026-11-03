//! The daemon's wire protocol: the JSON bodies of /resolve, /refresh and
//! /dead, and the answer to /resolve and /refresh. The daemon serves them
//! and [`Client`](super::Client) sends them.

use serde::{Deserialize, Serialize};

/// Body of /resolve, /refresh and /dead.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TargetRequest {
    #[serde(default)]
    pub target: String,
    /// Set by /dead when the failure will not clear on its own. Absent
    /// on older callers, which book the temporary backoff.
    #[serde(default)]
    pub permanent: bool,
    /// Byte the caller opened the file at. Absent means the start, which
    /// is where a scan probe reads. A later byte is a seek.
    #[serde(default)]
    pub offset: i64,
    /// Set by playback. A scan probe leaves it unset, so a cache hit on
    /// that path is not counted as another read.
    #[serde(default)]
    pub playback: bool,
}

/// The daemon's answer to /resolve and /refresh.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolved {
    pub direct: String,
    pub expires_at: i64,
    pub cached: bool,
}

//! The resident daemon: a persistent direct-link cache served over UDS.
//! Links are trusted until a playback failure reports them dead; expiry
//! timestamps are stored for information only.

mod breaker;
mod cache;
mod client;
mod daemon;
mod identity;
mod ledger;
mod markers;
mod quota;
mod watch;

pub use cache::{Cache, Entry, OpenError, Stats};
pub use client::{Client, ClientError};
pub use daemon::{DEFAULT_SOCK, Daemon, Resolved, TargetRequest};
pub use identity::Identity;
pub use markers::{MarkerDecision, MarkerOrigin, MarkerRecord, marker_digest};
pub use watch::{DEFAULT_LOG, detail_item, follow, log_path};

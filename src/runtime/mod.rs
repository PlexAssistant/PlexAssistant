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
mod parts;
mod quota;
mod tracks;
mod usage;
mod watch;

pub use cache::{Cache, DEFAULT_CACHE, Entry, OpenError, Stats, cache_path};
pub use client::{Client, ClientError};
pub use daemon::{DEFAULT_SOCK, Daemon, Resolved, TargetRequest};
pub use identity::Identity;
pub use markers::{MarkerDecision, MarkerOrigin, MarkerRecord, marker_digest};
pub use parts::PartFailure;
pub use tracks::{ChoiceUpdate, Remembered};
pub use watch::{DEFAULT_LOG, detail_item, follow, log_path};

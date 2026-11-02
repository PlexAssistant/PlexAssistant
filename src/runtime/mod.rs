//! The resident daemon: a persistent direct-link cache served over UDS.
//! Links are trusted until a playback failure reports them dead; expiry
//! timestamps are stored for information only.

mod breaker;
mod cache;
mod daemon;
mod ledger;
mod quota;

pub use cache::{Cache, Entry, OpenError, Stats};
pub use daemon::{DEFAULT_SOCK, Daemon, Resolved, TargetRequest};

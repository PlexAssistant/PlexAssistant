//! The resident daemon: a persistent direct-link cache served over UDS.
//! Links are trusted until a playback failure reports them dead; expiry
//! timestamps are stored for information only.

mod cache;
mod ledger;

pub use cache::{Cache, Entry, OpenError, Stats};

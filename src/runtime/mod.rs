//! The resident daemon: a persistent direct-link cache served over UDS.
//! Links are trusted until a playback failure reports them dead; expiry
//! timestamps are stored for information only.

// TODO(daemon): drop the allow once the daemon wires the breaker in
#[allow(dead_code)]
mod breaker;
mod cache;
mod ledger;
// TODO(daemon): drop the allow once the daemon wires the quota in
#[allow(dead_code)]
mod quota;

pub use cache::{Cache, Entry, OpenError, Stats};

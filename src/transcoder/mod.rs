//! Wraps the Plex Transcoder: strm inputs are swapped for cached direct
//! links before handing off to the original binary, so playback neither
//! re-resolves the gateway URL nor bypasses the cache.

mod rewrite;

pub use rewrite::{DaemonResolver, Resolver, Swap, rewrite_args};

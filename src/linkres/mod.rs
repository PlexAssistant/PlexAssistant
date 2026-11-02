//! Resolves .strm targets to direct links: follows 302 chains with UA
//! binding and reports link expiry.

mod expiry;
mod resolve;
mod strm;

pub use expiry::sniff_expiry;
pub use resolve::{DEFAULT_UA, Link, ResolveError, resolve};
pub use strm::{Target, parse_strm};

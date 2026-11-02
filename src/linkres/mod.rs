//! Resolves .strm targets to direct links: follows 302 chains with UA
//! binding and reports link expiry.

mod expiry;
mod strm;

pub use expiry::sniff_expiry;
pub use strm::{Target, parse_strm};

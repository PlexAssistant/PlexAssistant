//! Resolves .strm targets to direct links: follows 302 chains with UA
//! binding and reports link expiry.

mod strm;

pub use strm::{Target, parse_strm};

//! Implements the Plex Scanner argv protocol and STRM analysis: parse
//! .strm, follow redirects, probe with ffprobe, write the media rows.

mod extra;
mod parse;
mod probe;
mod spec;

pub use probe::{Format, ProbeError, ProbeResult, Stream, Tags, duration_ms, probe};
pub use spec::build_spec;

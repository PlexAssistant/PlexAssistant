//! Implements the Plex Scanner argv protocol and STRM analysis: parse
//! .strm, follow redirects, probe with ffprobe, write the media rows.

// TODO(analyze): drop the allow once build_spec uses extra_data
#[allow(dead_code)]
mod extra;
mod parse;
mod probe;

pub use probe::{Format, ProbeError, ProbeResult, Stream, Tags, duration_ms, probe};

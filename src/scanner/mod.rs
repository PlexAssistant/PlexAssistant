//! Implements the Plex Scanner argv protocol and STRM analysis: parse
//! .strm, follow redirects, probe with ffprobe, write the media rows.

mod analyze;
pub mod credits;
pub mod credits_model;
mod extra;
pub mod intro;
mod lang;
mod lang_table;
mod parse;
mod probe;
mod spec;

pub use analyze::{AnalyzeError, Options, analyze_item, is_strm, resolve_link};
pub use probe::{
    Disposition, Format, FormatTags, ProbeError, ProbeResult, Stream, Tags, duration_ms, probe,
};
pub use spec::build_spec;

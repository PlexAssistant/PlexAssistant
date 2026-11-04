//! Implements the Plex Scanner argv protocol and STRM analysis: parse
//! .strm, follow redirects, probe with ffprobe, write the media rows.

mod analyze;
pub mod credits;
mod extra;
pub mod intro;
mod lang;
mod lang_table;
mod parse;
mod probe;
mod redact;
mod spec;

pub use analyze::{
    AnalyzeError, Options, Probed, Refreshed, analyze_item, is_strm, probe_item, resolve_link,
    write_item,
};
pub use probe::{
    Disposition, Format, FormatTags, ProbeError, ProbeResult, Stream, Tags, duration_ms, probe,
};
pub use redact::redact_urls;
pub use spec::build_spec;

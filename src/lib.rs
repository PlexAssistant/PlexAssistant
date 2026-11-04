//! Native STRM support for Plex: the library behind the multi-call
//! `assistant` binary that replaces the Plex Scanner and Transcoder.
//!
//! # How the pieces fit
//!
//! Plex starts the scanner and the transcoder itself; [`install`] puts this
//! binary in their place, and [`cli`] tells the roles apart by `argv[0]`.
//!
//! - [`scanner`] answers a scan: it resolves each `.strm` target, probes the
//!   direct link with ffprobe, and writes the media rows through [`plexdb`].
//!   It also detects intro and credits markers. [`localize`] rewrites genre,
//!   mood and style labels when `ZH_LOCALIZE` asks for it.
//! - [`transcoder`] answers playback: it swaps a `.strm` input for a direct
//!   link, then runs the original transcoder.
//! - [`runtime`] is the resident daemon both of them ask for links. It keeps
//!   the link cache and the ledgers: failed targets, markers, remembered
//!   track choices and cloud usage.
//! - [`linkres`] follows a target's redirects to the direct link, and
//!   [`plexdb`] reads and writes Plex's library database.
//!
//! `src/bin/strmock` is a fake cloud drive for the dev rig, not part of the
//! image.

pub mod cli;
pub mod install;
pub mod linkres;
pub mod localize;
pub mod plexdb;
pub mod runtime;
pub mod scanner;
pub mod transcoder;

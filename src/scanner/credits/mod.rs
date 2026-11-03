//! Credits detection over the tail of a file.
//!
//! Three detectors, tried in an order the user picks:
//! - [`black`]: ffmpeg's blackdetect finds a long black window.
//! - [`graph`]: the text-detection model Plex already stored scores frames.
//! - [`edges`]: edge density finds text over live picture.
//!
//! Plex invokes credits detection with its own argv, so the choice is an
//! environment variable rather than a flag.

pub mod assets;
mod black;
mod edges;
mod frames;
mod graph;
pub mod net;

use std::path::Path;

/// Only the tail is scanned: credits live at the end, and walking a
/// whole hour of 4K over a cloud link costs real bandwidth; the window
/// size follows what real credits look like.
const TAIL_SECS: f64 = 600.0;

/// `ASSISTANT_CREDITS`. Unset and anything unrecognised is [`Detector::Auto`].
pub const ENV: &str = "ASSISTANT_CREDITS";

/// The detector a scan should use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detector {
    /// Black window, then the graph, then edges.
    Auto,
    /// Black window, then edges. The graph is not loaded.
    Heuristic,
    /// The graph only. A missing model or a failed run writes nothing.
    Model,
}

impl Detector {
    /// The choice named by `value`. Empty and unknown names are `Auto`.
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("heuristic") => Self::Heuristic,
            Some("model") => Self::Model,
            _ => Self::Auto,
        }
    }

    /// The choice named by [`ENV`].
    pub fn from_env() -> Self {
        Self::parse(std::env::var(ENV).ok().as_deref())
    }
}

/// The credits window of `input` as `(start_ms, end_ms)`, found by the
/// detectors `detector` names, in order. `duration_secs` places the seek
/// into the tail.
pub async fn detect(
    detector: Detector,
    ffmpeg: &str,
    input: &str,
    ua: &str,
    duration_secs: f64,
) -> Option<(i64, i64)> {
    if detector == Detector::Model {
        return detect_with_graph(ffmpeg, input, ua, duration_secs).await;
    }
    if let Some(window) = black::detect(ffmpeg, input, ua, duration_secs).await {
        return Some(window);
    }
    if detector == Detector::Auto
        && let Some(window) = detect_with_graph(ffmpeg, input, ua, duration_secs).await
    {
        return Some(window);
    }
    edges::detect(ffmpeg, input, ua, duration_secs).await
}

/// The graph's credits window, when the stored model loads and finds one.
///
/// A missing model, a graph that will not run, or a failed extraction
/// all return `None`. In `auto` the caller then tries the edge detector;
/// in `model` a miss writes nothing.
async fn detect_with_graph(
    ffmpeg: &str,
    input: &str,
    ua: &str,
    duration_secs: f64,
) -> Option<(i64, i64)> {
    let path = assets::path(Path::new(assets::DEFAULT_CONFIG));
    let stored = assets::load(&path).await.ok()??;
    let plan = net::load(&stored.path).ok()?;
    graph::detect(&plan, ffmpeg, input, ua, duration_secs)
        .await
        .ok()?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_three_names() {
        assert_eq!(Detector::parse(Some("heuristic")), Detector::Heuristic);
        assert_eq!(Detector::parse(Some("model")), Detector::Model);
        assert_eq!(Detector::parse(Some("auto")), Detector::Auto);
    }

    #[test]
    fn unknown_and_empty_stay_on_auto() {
        assert_eq!(Detector::parse(None), Detector::Auto);
        assert_eq!(Detector::parse(Some("")), Detector::Auto);
        assert_eq!(Detector::parse(Some("graph")), Detector::Auto);
        assert_eq!(Detector::parse(Some("  heuristic  ")), Detector::Heuristic);
    }
}

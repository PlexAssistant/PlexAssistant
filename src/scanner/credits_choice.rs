//! Which credits detector a scan uses.
//!
//! Plex invokes credits detection with its own argv, so the choice is an
//! environment variable rather than a flag. `auto` is the default: the
//! black window first, then the graph when the stored model loads, then
//! the edge detector.

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

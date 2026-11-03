//! Credits detection from the published graph's score map.
//!
//! Each frame is one 320×320 sample. A frame counts as text when enough
//! of its score cells clear the threshold, and a credits window is the
//! longest run of those frames that still clears the minimum Plex honors.
//! The geometry output is not read: the score map is enough to place the
//! window, and a box decoder can tighten it later.

use super::credits::TAIL_WINDOW_SECS;
use super::credits_net::Plan;
use super::frame_source::{self, Frame, FrameSpec};

/// A score cell counts as text at this confidence.
const SCORE_THRESHOLD: f32 = 0.5;

/// A frame counts as text when this share of its score cells clear
/// [`SCORE_THRESHOLD`]. One credit line lights a handful of the 80×80
/// cells, which is a few tenths of a percent.
const FRAME_SHARE: f32 = 0.001;

/// Below-threshold frames tolerated inside a run. Scrolling text dips
/// as a line leaves the frame.
const MAX_GAP_FRAMES: usize = 3;

/// Shortest run that can be a marker, in seconds. Plex ignores anything
/// shorter.
const MIN_RUN_SECS: f64 = 15.0;

/// Scores the tail with `plan` and returns the credits window as
/// `(start_ms, end_ms)`.
///
/// `duration_secs` places the seek. A missing window is `Ok(None)`; a
/// failed extraction or a frame the plan rejects is an error, so the
/// caller can fall back without mistaking it for "no credits".
pub async fn detect(
    plan: &Plan,
    ffmpeg: &str,
    input: &str,
    ua: &str,
    duration_secs: f64,
) -> std::io::Result<Option<(i64, i64)>> {
    let tail_start = (duration_secs - TAIL_WINDOW_SECS).max(0.0);
    let frames = frame_source::extract_tail(ffmpeg, input, ua, tail_start, FrameSpec::RGB).await?;
    let shares = frames
        .iter()
        .map(|frame| plan.score(&frame.bytes, SCORE_THRESHOLD))
        .collect::<Result<Vec<_>, _>>()
        .map_err(std::io::Error::other)?;
    Ok(window_in(&shares, &frames, FrameSpec::RGB.step))
}

/// The longest text run in already-scored frames, as `(start_ms, end_ms)`.
///
/// `shares` is the hot-cell share of each frame, paired with `frames` by
/// index. A run's end is one step past its last text frame, which is
/// where the next sample would have been.
pub fn window_in(shares: &[f32], frames: &[Frame], step: f64) -> Option<(i64, i64)> {
    let min_frames = (MIN_RUN_SECS / step).ceil() as usize;
    if frames.len() < min_frames || shares.len() != frames.len() {
        return None;
    }
    let text: Vec<bool> = shares.iter().map(|share| *share >= FRAME_SHARE).collect();

    let mut best: Option<(usize, usize)> = None;
    let mut run_start = 0;
    let mut run_len = 0usize;
    let mut gap = 0usize;
    for i in 0..=text.len() {
        let hot = i < text.len() && text[i];
        if hot {
            if run_len == 0 {
                run_start = i;
            }
            run_len += 1;
            gap = 0;
        } else if run_len > 0 {
            gap += 1;
            if gap > MAX_GAP_FRAMES {
                let end = i - gap;
                consider(&mut best, run_start, end, min_frames);
                run_len = 0;
                gap = 0;
            }
        }
    }
    if run_len > 0 {
        consider(&mut best, run_start, text.len(), min_frames);
    }

    let (start, end) = best?;
    let start_ms = (frames[start].pts * 1000.0) as i64;
    let end_ms = ((frames[start].pts + (end - start) as f64 * step) * 1000.0) as i64;
    Some((start_ms, end_ms))
}

fn consider(best: &mut Option<(usize, usize)>, start: usize, end: usize, min_frames: usize) {
    let len = end - start;
    if len < min_frames {
        return;
    }
    let longer = best.is_none_or(|(best_start, best_end)| len > best_end - best_start);
    if longer {
        *best = Some((start, end));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(pts: f64) -> Frame {
        Frame {
            pts,
            bytes: Vec::new(),
        }
    }

    #[test]
    fn finds_the_sustained_hot_run() {
        let frames: Vec<Frame> = (0..30).map(|i| frame(i as f64 * 2.0)).collect();
        let mut shares = vec![0.0; 30];
        for share in &mut shares[15..] {
            *share = 0.004;
        }
        assert_eq!(window_in(&shares, &frames, 2.0), Some((30_000, 60_000)));
    }

    #[test]
    fn tolerates_a_short_dip_inside_the_run() {
        let frames: Vec<Frame> = (0..32).map(|i| frame(i as f64 * 2.0)).collect();
        let mut shares = vec![0.0; 32];
        for share in &mut shares[15..] {
            *share = 0.004;
        }
        shares[20] = 0.0;
        shares[21] = 0.0;
        assert_eq!(window_in(&shares, &frames, 2.0), Some((30_000, 64_000)));
    }

    #[test]
    fn rejects_a_run_shorter_than_the_minimum() {
        // seven frames is 14 seconds, under the 15-second minimum
        let frames: Vec<Frame> = (0..22).map(|i| frame(i as f64 * 2.0)).collect();
        let mut shares = vec![0.0; 22];
        for share in &mut shares[15..] {
            *share = 0.004;
        }
        assert_eq!(window_in(&shares, &frames, 2.0), None);
    }

    #[test]
    fn rebases_onto_the_file_when_the_tail_was_seeked() {
        let frames: Vec<Frame> = (0..30).map(|i| frame(1_000.0 + i as f64 * 2.0)).collect();
        let mut shares = vec![0.0; 30];
        for share in &mut shares[15..] {
            *share = 0.004;
        }
        assert_eq!(
            window_in(&shares, &frames, 2.0),
            Some((1_030_000, 1_060_000))
        );
    }
}

//! Credits detection from the published graph's score map.
//!
//! Each frame is one 320×320 sample. A frame counts as text when enough
//! of its score cells clear the threshold, and a credits window is the
//! longest run of those frames that still clears the minimum Plex honors.
//! The geometry output places the text inside the first and last frame of
//! that run, which pulls the window's ends off the two-second sample grid.

use super::TAIL_SECS;
use super::frames::{self, Frame, FrameSpec};
use super::net::{self, Plan};

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
    let tail_start = (duration_secs - TAIL_SECS).max(0.0);
    let frames = frames::extract_tail(ffmpeg, input, ua, tail_start, FrameSpec::RGB).await?;
    let square: Vec<Vec<u8>> = frames
        .iter()
        .filter_map(|frame| {
            frames::pad_square(&frame.bytes, FrameSpec::RGB.width, FrameSpec::RGB.height)
        })
        .collect();
    let scored = square
        .iter()
        .map(|frame| plan.maps(frame))
        .collect::<Result<Vec<_>, _>>()
        .map_err(std::io::Error::other)?;
    let shares: Vec<f32> = scored
        .iter()
        .map(|(score, _)| hot_share(score, SCORE_THRESHOLD))
        .collect();
    // The vertical centre of the text in the first and last frame of the run.
    // Sampled frames are two seconds apart, so the window otherwise starts and
    // ends on a sample; the box says where in that frame the text sits.
    let centres: Vec<Option<f32>> = scored
        .iter()
        .map(|(score, geometry)| text_centre(score, geometry, SCORE_THRESHOLD))
        .collect();
    Ok(tighten(
        window_in(&shares, &frames, FrameSpec::RGB.step),
        &centres,
        &frames,
        FrameSpec::RGB.step,
    ))
}

/// The share of `score` above `threshold`.
fn hot_share(score: &[f32], threshold: f32) -> f32 {
    if score.is_empty() {
        return 0.0;
    }
    let hot = score.iter().filter(|value| **value > threshold).count();
    hot as f32 / score.len() as f32
}

/// The vertical centre of the text in one frame, in canvas pixels.
///
/// Nothing when the frame has no hot cell. The centre is what moves as
/// credits scroll, so it is what places the window inside a sampled frame.
fn text_centre(score: &[f32], geometry: &[f32], threshold: f32) -> Option<f32> {
    let (y0, y1) = net::bounds_of(score, geometry, threshold).map(|(_, y0, _, y1)| (y0, y1))?;
    Some((y0 + y1) / 2.0)
}

/// Moves a window's ends off the sample grid when the text is scrolling.
///
/// `centres` parallels `frames`: the text's vertical centre in that frame, in
/// canvas pixels, or nothing when the frame has no text. The window otherwise
/// starts and ends on a sample, up to one step early or late. How far the text
/// moved over the run says which it is, and a run whose text barely moved is
/// left alone: a credit line that fades in standing still is already where the
/// samples say it is.
fn tighten(
    window: Option<(i64, i64)>,
    centres: &[Option<f32>],
    frames: &[Frame],
    step: f64,
) -> Option<(i64, i64)> {
    let (start_ms, end_ms) = window?;
    let start = frames
        .iter()
        .position(|frame| frame.pts * 1000.0 >= start_ms as f64)?;
    let end = frames
        .iter()
        .rposition(|frame| frame.pts * 1000.0 < end_ms as f64)?;
    let (Some(from), Some(to)) = (
        centres.get(start).copied().flatten(),
        centres.get(end).copied().flatten(),
    ) else {
        return Some((start_ms, end_ms));
    };
    // A line that scrolls off one edge comes back on the other, so the jump
    // between two samples can be most of the frame and says nothing about the
    // direction. The steps that stay small are the scrolling itself.
    let moved = centres
        .windows(2)
        .filter_map(|pair| Some(pair[1]? - pair[0]?))
        .filter(|step| step.abs() < net::CANVAS as f32 * 0.5)
        .sum::<f32>();
    // Less than a tenth of the frame over the whole run is a line standing
    // still, or noise in the box, not a scroll.
    if moved.abs() < net::CANVAS as f32 * 0.1 {
        return Some((start_ms, end_ms));
    }
    // Both ends are late by the fraction of the frame the line there had not
    // yet crossed. Upward scrolling crosses from the bottom, so that fraction
    // is the line's height; downward scrolling crosses from the top, so it is
    // the remainder. The end has no sample of its own: it sits one step past
    // the last frame, and that frame's fraction says how much of the step was
    // real text.
    let correct = |centre: f32| -> i64 {
        let crossed = centre / net::CANVAS as f32;
        let pending = if moved < 0.0 { crossed } else { 1.0 - crossed };
        (pending.clamp(0.0, 1.0) as f64 * step * 1000.0) as i64
    };
    Some(((start_ms + correct(from)).max(0), end_ms - correct(to)))
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

    /// A run hot from frame 15 on, which the samples place at 30s to 60s.
    ///
    /// Only the first and last hot frame carry a centre; the shift is read off
    /// those two, so the frames between them do not matter here.
    fn scrolling(first: f32, last: f32) -> Option<(i64, i64)> {
        let frames: Vec<Frame> = (0..30).map(|i| frame(i as f64 * 2.0)).collect();
        let mut shares = vec![0.0; 30];
        for share in &mut shares[15..] {
            *share = 0.004;
        }
        let mut centres = vec![None; 30];
        let span = (centres.len() - 1 - 15) as f32;
        for (i, centre) in centres.iter_mut().enumerate().skip(15) {
            let along = (i - 15) as f32 / span;
            *centre = Some(first + (last - first) * along);
        }
        tighten(window_in(&shares, &frames, 2.0), &centres, &frames, 2.0)
    }

    #[test]
    fn holds_a_line_that_is_standing_still() {
        assert_eq!(scrolling(160.0, 160.0), Some((30_000, 60_000)));
    }

    #[test]
    fn delays_the_start_when_text_scrolls_up_from_the_bottom() {
        // The first line is centred at 256 of 320, so it entered 1.6s after
        // the sample, and the last line, centred at 64, accounts for 0.4s of
        // the step past the final sample.
        let (start, end) = scrolling(256.0, 64.0).unwrap();
        assert_eq!(start, 30_000 + 1_600);
        assert_eq!(end, 60_000 - 400);
    }

    #[test]
    fn brings_the_start_forward_when_text_scrolls_down() {
        // Entering from the top: centred at 64 of 320 means 1.6s of the step
        // had already passed, so the sample was that late. The last line,
        // centred at 256, accounts for 0.4s past the final sample.
        let (start, end) = scrolling(64.0, 256.0).unwrap();
        // A centre that is not a round fraction of the frame rounds the shift
        // by a millisecond.
        assert!((start - (30_000 + 1_600)).abs() <= 1, "{start}");
        assert!((end - (60_000 - 400)).abs() <= 1, "{end}");
    }

    #[test]
    fn leaves_the_window_when_no_frame_has_a_box() {
        let frames: Vec<Frame> = (0..30).map(|i| frame(i as f64 * 2.0)).collect();
        let mut shares = vec![0.0; 30];
        for share in &mut shares[15..] {
            *share = 0.004;
        }
        let window = window_in(&shares, &frames, 2.0);
        assert_eq!(tighten(window, &vec![None; 30], &frames, 2.0), window);
    }
}

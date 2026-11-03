//! Visual credits detection for frames where the black-window heuristic
//! comes up empty: text overlays over live picture. Extracts edge maps
//! from the tail window and finds sustained runs of high edge density
//! that stand out from the content's baseline.

use super::frame_source::{self, Frame, FrameSpec};

/// Frames from the tail window to sample.
const TAIL_SECS: f64 = 600.0;

/// A frame counts as "has text overlay" when its edge density is this
/// many times the content's running median. Text adds structured
/// high-contrast edges; the exact ratio varies by content type.
const DENSITY_RATIO: f64 = 1.08;

/// Consecutive below-threshold frames tolerated inside a text run
/// (scrolling text dips as it crosses screen edges).
const MAX_GAP_FRAMES: usize = 3;

/// Minimum text frames (at 0.5 fps) for a valid run.
const MIN_TEXT_FRAMES: usize = 8; // 16 seconds

/// The visual fallback for credits detection: extract grayscale edge
/// maps from the tail, find the longest contiguous run of frames whose
/// edge density exceeds the content baseline, and return it as
/// (start_ms, end_ms).
pub async fn detect_credits_visual(
    ffmpeg: &str,
    input: &str,
    ua: &str,
    duration_secs: f64,
) -> Option<(i64, i64)> {
    let tail_start = (duration_secs - TAIL_SECS).max(0.0);
    let frames = frame_source::extract_tail(ffmpeg, input, ua, tail_start, FrameSpec::EDGES)
        .await
        .ok()?;
    detect_in_frames(&frames, FrameSpec::EDGES.step)
}

/// Finds the longest sustained text run in already-extracted edge frames.
/// Split from the extraction so the window search can be tested without
/// ffmpeg. `step` is the seconds between frames; a run's end is one step
/// past its last frame, which is where the next sample would have been.
fn detect_in_frames(frames: &[Frame], step: f64) -> Option<(i64, i64)> {
    let frame_bytes = frames.first().map(|f| f.bytes.len()).unwrap_or(0);
    if frames.len() < MIN_TEXT_FRAMES || frame_bytes == 0 {
        eprintln!("credits visual: only {} frames", frames.len());
        return None;
    }

    // edge density per frame: fraction of pixels above a brightness cutoff
    let densities: Vec<f64> = frames
        .iter()
        .map(|f| {
            let edges = f.bytes.iter().filter(|&&p| p > 60).count();
            edges as f64 / frame_bytes as f64
        })
        .collect();

    // baseline = median; text frames are denser than baseline * ratio
    let mut sorted = densities.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[sorted.len() / 2];
    let threshold = median * DENSITY_RATIO;

    // find the longest run above threshold, tolerating gaps for
    // scrolling text that dips as it crosses screen edges
    let has_text: Vec<bool> = densities.iter().map(|&d| d > threshold).collect();
    let mut best: Option<(usize, usize)> = None;
    let mut run_start = 0;
    let mut run_len = 0usize;
    let mut gap = 0usize;

    for i in 0..=has_text.len() {
        let text = i < has_text.len() && has_text[i];
        if text {
            if run_len == 0 {
                run_start = i;
            }
            run_len += 1;
            gap = 0;
        } else if run_len > 0 {
            gap += 1;
            if gap > MAX_GAP_FRAMES {
                // close the run
                let end = i - gap;
                let better = best.is_none_or(|(bs, be)| run_len > be - bs);
                if run_len >= MIN_TEXT_FRAMES && better {
                    best = Some((run_start, end));
                }
                run_len = 0;
                gap = 0;
            }
        }
    }
    // trailing run; the closing branch above reports an exclusive end, so
    // this one does too
    if run_len >= MIN_TEXT_FRAMES {
        let end = has_text.len();
        let better = best.is_none_or(|(bs, be)| run_len > be - bs);
        if better {
            best = Some((run_start, end));
        }
    }

    let (start_f, end_f) = best?;
    let start = frames[start_f].pts * 1000.0;
    // end_f is exclusive: the run covers `end_f - start_f` frames
    let end = (frames[start_f].pts + (end_f - start_f) as f64 * step) * 1000.0;
    let n = frames.len();
    println!("credits visual: {n} frames, median density {median:.4}, run {start_f}-{end_f}");
    Some((start as i64, end as i64))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIXELS: usize = 320 * 320;

    /// A flat edge map: `bright` pixels set, the rest dark. Density is
    /// `bright / PIXELS`, which is what the detector thresholds on.
    fn frame(pts: f64, bright: usize) -> Frame {
        let mut bytes = vec![0u8; PIXELS];
        for pixel in bytes.iter_mut().take(bright) {
            *pixel = 255;
        }
        Frame { pts, bytes }
    }

    #[test]
    fn finds_the_sustained_dense_run() {
        // 20 quiet frames, then 10 dense ones: the run is the only thing
        // above the median, and it clears the 8-frame minimum
        let mut frames: Vec<Frame> = (0..20).map(|i| frame(i as f64 * 2.0, 1_000)).collect();
        frames.extend((20..30).map(|i| frame(i as f64 * 2.0, 20_000)));

        let (start, end) = detect_in_frames(&frames, 2.0).expect("text run");
        assert_eq!(start, 40_000);
        assert_eq!(end, 60_000);
    }

    #[test]
    fn tolerates_a_short_dip_inside_the_run() {
        let mut frames: Vec<Frame> = (0..20).map(|i| frame(i as f64 * 2.0, 1_000)).collect();
        // two quiet frames in the middle stay inside the gap allowance
        for i in 20..32 {
            let bright = if i == 24 || i == 25 { 1_000 } else { 20_000 };
            frames.push(frame(i as f64 * 2.0, bright));
        }

        let (start, end) = detect_in_frames(&frames, 2.0).expect("text run");
        assert_eq!(start, 40_000);
        assert_eq!(end, 64_000);
    }

    #[test]
    fn rejects_a_run_shorter_than_the_minimum() {
        let mut frames: Vec<Frame> = (0..20).map(|i| frame(i as f64 * 2.0, 1_000)).collect();
        frames.extend((20..25).map(|i| frame(i as f64 * 2.0, 20_000)));
        assert_eq!(detect_in_frames(&frames, 2.0), None);
    }

    #[test]
    fn rebases_onto_the_file_when_the_tail_was_seeked() {
        let mut frames: Vec<Frame> = (0..20)
            .map(|i| frame(1_000.0 + i as f64 * 2.0, 1_000))
            .collect();
        frames.extend((20..30).map(|i| frame(1_000.0 + i as f64 * 2.0, 20_000)));

        let (start, end) = detect_in_frames(&frames, 2.0).expect("text run");
        assert_eq!(start, 1_040_000);
        assert_eq!(end, 1_060_000);
    }
}

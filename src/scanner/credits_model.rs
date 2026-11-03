//! Visual credits detection for frames where the black-window heuristic
//! comes up empty: text overlays over live picture. Extracts edge maps
//! from the tail window and finds sustained runs of high edge density
//! that stand out from the content's baseline.

use tokio::process::Command;

/// Seconds per sampled frame (ffmpeg fps filter).
const FRAME_STEP: f64 = 2.0;

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

    let mut args: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
    ];
    if input.starts_with("http://") || input.starts_with("https://") {
        args.push("-headers".into());
        args.push(format!("User-Agent: {ua}\r\n"));
    }
    if tail_start > 0.0 {
        args.push("-ss".into());
        args.push(format!("{tail_start:.3}"));
    }
    args.extend([
        "-i".into(),
        input.to_owned(),
        "-an".into(),
        "-vf".into(),
        // sobel edges at 320px grayscale, 0.5 fps sampling
        "fps=0.5,scale=320:-2,format=gray,edgedetect=low=0.08:high=0.2".into(),
        "-f".into(),
        "rawvideo".into(),
        "-".into(),
    ]);

    let out = Command::new(ffmpeg)
        .args(&args)
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    let stdout = out.stdout;
    if stdout.is_empty() {
        eprintln!("credits visual: no frames extracted");
        return None;
    }

    // each frame is a variable-width grayscale strip; we need the width
    // to split frames. scale=320:-2 preserves aspect, so height varies.
    // re-run with fixed square padding to make frame size predictable.
    // for now, detect width from the first frame boundary is fragile;
    // simpler: use scale=320:320 with pad.
    let _ = stdout; // fall through to padded version below
    detect_with_padding(ffmpeg, input, ua, tail_start).await
}

/// Fixed-size version: pad to 320x320 so each frame is exactly
/// 320 * 320 = 102400 bytes of grayscale.
async fn detect_with_padding(
    ffmpeg: &str,
    input: &str,
    ua: &str,
    tail_start: f64,
) -> Option<(i64, i64)> {
    const W: usize = 320;
    const H: usize = 320;
    const FRAME_BYTES: usize = W * H;

    let mut args: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
    ];
    if input.starts_with("http://") || input.starts_with("https://") {
        args.push("-headers".into());
        args.push(format!("User-Agent: {ua}\r\n"));
    }
    if tail_start > 0.0 {
        args.push("-ss".into());
        args.push(format!("{tail_start:.3}"));
    }
    args.extend([
        "-i".into(),
        input.to_owned(),
        "-an".into(),
        "-vf".into(),
        "fps=0.5,scale=320:320:force_original_aspect_ratio=decrease,pad=320:320:(ow-iw)/2:(oh-ih)/2,format=gray,edgedetect=low=0.08:high=0.2".into(),
        "-f".into(),
        "rawvideo".into(),
        "-".into(),
    ]);

    let out = Command::new(ffmpeg)
        .args(&args)
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;

    let frames: Vec<&[u8]> = out
        .stdout
        .chunks(FRAME_BYTES)
        .filter(|c| c.len() == FRAME_BYTES)
        .collect();
    if frames.len() < MIN_TEXT_FRAMES {
        eprintln!("credits visual: only {} frames", frames.len());
        return None;
    }

    // edge density per frame: fraction of pixels above a brightness cutoff
    let densities: Vec<f64> = frames
        .iter()
        .map(|f| {
            let edges = f.iter().filter(|&&p| p > 60).count();
            edges as f64 / FRAME_BYTES as f64
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
    // trailing run
    if run_len >= MIN_TEXT_FRAMES {
        let end = has_text.len() - gap;
        let better = best.is_none_or(|(bs, be)| run_len > be - bs);
        if better {
            best = Some((run_start, end));
        }
    }

    let (start_f, end_f) = best?;
    let start = (tail_start + start_f as f64 * FRAME_STEP) * 1000.0;
    let end = (tail_start + end_f as f64 * FRAME_STEP) * 1000.0;
    let n = frames.len();
    println!("credits visual: {n} frames, median density {median:.4}, run {start_f}-{end_f}");
    Some((start as i64, end as i64))
}

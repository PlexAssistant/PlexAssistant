//! Single-pass frame extraction for credits detection.
//!
//! One ffmpeg invocation seeks into the tail, samples at a fixed rate,
//! scales to a fixed canvas and streams rawvideo on stdout. Callers cut
//! the byte stream into frames; there is no intermediate file and no
//! second pass over the source.

use std::io;

use tokio::process::Command;

/// One sampled frame: raw pixels and where it sits in the source, in
/// seconds from the start of the file.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub pts: f64,
    pub bytes: Vec<u8>,
}

/// Geometry and sampling of one extraction.
#[derive(Debug, Clone, Copy)]
pub struct FrameSpec {
    /// Canvas width in pixels.
    pub width: usize,
    /// Canvas height in pixels. Frames are letterboxed onto this canvas,
    /// so every frame is exactly `width * height * bytes_per_pixel`.
    pub height: usize,
    /// Bytes per pixel in the output format (`1` for gray, `3` for rgb24).
    pub bytes_per_pixel: usize,
    /// ffmpeg `-vf` chain. Must end in a pixel format that matches
    /// `bytes_per_pixel`, and must produce `width`×`height` frames.
    pub filter: &'static str,
    /// Seconds between sampled frames (`1 / fps`).
    pub step: f64,
}

impl FrameSpec {
    /// Edge maps for the visual credits fallback: 320×320 gray, one byte
    /// per pixel, sampled twice a second.
    pub const EDGES: Self = Self {
        width: 320,
        height: 320,
        bytes_per_pixel: 1,
        filter: "fps=0.5,scale=320:320:force_original_aspect_ratio=decrease,\
                  pad=320:320:(ow-iw)/2:(oh-ih)/2,format=gray,\
                  edgedetect=low=0.08:high=0.2",
        step: 2.0,
    };

    fn frame_bytes(self) -> usize {
        self.width * self.height * self.bytes_per_pixel
    }
}

/// Extracts the tail of `input` as raw frames.
///
/// `tail_start` is a seek offset in seconds; frame timestamps are rebased
/// onto the source, so callers see file time rather than time since the
/// seek. A short trailing chunk (ffmpeg closed mid-frame) is dropped.
pub async fn extract_tail(
    ffmpeg: &str,
    input: &str,
    ua: &str,
    tail_start: f64,
    spec: FrameSpec,
) -> io::Result<Vec<Frame>> {
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
    ];
    if input.starts_with("http://") || input.starts_with("https://") {
        // ffmpeg has no -user_agent option; the header form carries it
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
        "-sn".into(),
        "-dn".into(),
        "-map".into(),
        "0:v:0".into(),
        "-vf".into(),
        spec.filter.into(),
        "-f".into(),
        "rawvideo".into(),
        "-".into(),
    ]);

    let out = Command::new(ffmpeg)
        .args(&args)
        .kill_on_drop(true)
        .output()
        .await?;

    let frame_bytes = spec.frame_bytes();
    let base = tail_start.max(0.0);
    let frames = out
        .stdout
        .chunks(frame_bytes)
        .filter(|chunk| chunk.len() == frame_bytes)
        .enumerate()
        .map(|(i, chunk)| Frame {
            pts: base + i as f64 * spec.step,
            bytes: chunk.to_vec(),
        })
        .collect();
    Ok(frames)
}

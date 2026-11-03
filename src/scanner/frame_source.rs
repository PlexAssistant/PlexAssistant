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
    /// Set when the filter keeps the picture's aspect, so the frame is
    /// `width` wide and the height is whatever the picture scaled to.
    /// `frame_bytes` is then this many bytes, and `height` is unused.
    pub proportional_bytes: usize,
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
        proportional_bytes: 0,
        filter: "fps=0.5,scale=320:320:force_original_aspect_ratio=decrease,\
                  pad=320:320:(ow-iw)/2:(oh-ih)/2,format=gray,\
                  edgedetect=low=0.08:high=0.2",
        step: 2.0,
    };

    /// RGB frames for the credits graph: 320 wide, height follows the
    /// picture, three bytes per pixel, sampled twice a second.
    ///
    /// The graph wants a square. Padding here would bake the bars into the
    /// sample and move the text; the caller pads each frame onto the square
    /// before scoring, so the same bytes can be compared with and without
    /// the bars.
    pub const RGB: Self = Self {
        width: 320,
        height: 180,
        bytes_per_pixel: 3,
        // 320×180×3. A 16:9 picture scaled to 320 wide with even height
        // lands here; other aspects need their own spec.
        proportional_bytes: 320 * 180 * 3,
        filter: "fps=0.5,scale=320:-2,format=rgb24",
        step: 2.0,
    };

    fn frame_bytes(self) -> usize {
        if self.proportional_bytes > 0 {
            return self.proportional_bytes;
        }
        self.width * self.height * self.bytes_per_pixel
    }
}

/// Pads one proportionally scaled frame onto a square canvas.
///
/// `rgb` is `width`×`height`×3. The frame is centred; the bars are black.
/// A frame that is already square is returned unchanged. `width` or
/// `height` of zero returns nothing.
pub fn pad_square(rgb: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || rgb.len() != width * height * 3 {
        return None;
    }
    let side = width.max(height);
    if width == height {
        return Some(rgb.to_vec());
    }
    let mut canvas = vec![0u8; side * side * 3];
    let x0 = (side - width) / 2;
    let y0 = (side - height) / 2;
    for row in 0..height {
        let src = row * width * 3;
        let dst = ((y0 + row) * side + x0) * 3;
        canvas[dst..dst + width * 3].copy_from_slice(&rgb[src..src + width * 3]);
    }
    Some(canvas)
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
    if frame_bytes == 0 {
        return Ok(Vec::new());
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wide_frame_is_centred_on_the_square() {
        // 4×2 red. The square is 4×4, so one black row sits above and below.
        let rgb = vec![
            255u8, 0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0, 255,
            0,
        ];
        let canvas = pad_square(&rgb, 4, 2).unwrap();
        assert_eq!(canvas.len(), 4 * 4 * 3);
        assert_eq!(&canvas[..12], &[0u8; 12]);
        assert_eq!(&canvas[12..24], &rgb[..12]);
        assert_eq!(&canvas[24..36], &rgb[12..]);
        assert_eq!(&canvas[36..], &[0u8; 12]);
    }

    #[test]
    fn a_square_frame_is_unchanged() {
        let rgb = vec![1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        assert_eq!(pad_square(&rgb, 2, 2).unwrap(), rgb);
    }

    #[test]
    fn a_short_buffer_is_not_a_frame() {
        assert!(pad_square(&[0, 1, 2], 2, 2).is_none());
    }
}

use std::fmt::Write;

use super::parse::parse_fps;
use super::probe::Stream;

/// Composes the stream extra_data the way the official analysis does: a
/// JSON object of ma: fields plus a url-encoded copy. PMS derives the
/// client's displayTitle ("2160p HDR10", ...) from them. The output must
/// stay byte-identical to what the Go implementation wrote.
pub(super) fn build_video_extra(s: &Stream) -> String {
    // already in key order, which url.Values.Encode also sorted by
    let fields = [
        ("ma:bitDepth", video_bit_depth(s)),
        ("ma:chromaLocation", s.chroma_location.clone()),
        (
            "ma:chromaSubsampling",
            chroma_subsampling(&s.pix_fmt).to_owned(),
        ),
        ("ma:codedHeight", itoa_or(s.coded_height, s.height)),
        ("ma:codedWidth", itoa_or(s.coded_width, s.width)),
        ("ma:colorPrimaries", s.color_primaries.clone()),
        ("ma:colorRange", s.color_range.clone()),
        ("ma:colorSpace", s.color_space.clone()),
        ("ma:colorTrc", s.color_transfer.clone()),
        ("ma:frameRate", format_frame_rate(&s.frame_rate)),
        ("ma:hasScalingMatrix", "0".to_owned()),
        ("ma:height", itoa_or(s.height, 0)),
        ("ma:level", itoa_or(s.level, 0)),
        ("ma:profile", s.profile.to_lowercase()),
        ("ma:refFrames", itoa_or(s.refs, 0)),
        ("ma:scanType", "progressive".to_owned()),
        ("ma:width", itoa_or(s.width, 0)),
    ];

    let mut json = String::from("{");
    let mut query = String::new();
    for (key, value) in fields.iter().filter(|(_, v)| !v.is_empty()) {
        if !query.is_empty() {
            json.push(',');
            query.push('&');
        }
        // for these values Go's %q and JSON escaping coincide
        let _ = write!(json, "{}:{}", json_string(key), json_string(value));
        let _ = write!(query, "{}={}", query_escape(key), query_escape(value));
    }
    let _ = write!(json, ",\"url\":{}}}", json_string(&query));
    json
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).expect("strings always serialize")
}

/// Go's url.QueryEscape: unreserved bytes stay, space becomes '+',
/// everything else is %XX. Unlike form_urlencoded, '~' stays and '*' is
/// escaped.
fn query_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

fn itoa_or(v: i64, fallback: i64) -> String {
    match if v == 0 { fallback } else { v } {
        0 => String::new(),
        n => n.to_string(),
    }
}

/// Prefers bits_per_raw_sample, then the depth encoded in pix_fmt, else
/// 8 for any known format.
fn video_bit_depth(s: &Stream) -> String {
    if !s.bit_depth.is_empty() && s.bit_depth != "N/A" {
        return s.bit_depth.clone();
    }
    match pix_fmt_depth(&s.pix_fmt) {
        Some(depth) => depth.to_string(),
        None if s.pix_fmt.is_empty() => String::new(),
        None => "8".to_owned(),
    }
}

/// FFmpeg names every format deeper than 8 bits with an endianness
/// suffix ("yuv420p10le", "yuv422p12be", "p010le"); the digits before it
/// are the depth. Digits without that suffix belong to the name itself,
/// as in "nv12", which is 8-bit.
fn pix_fmt_depth(pix_fmt: &str) -> Option<u32> {
    let base = pix_fmt
        .strip_suffix("le")
        .or_else(|| pix_fmt.strip_suffix("be"))?;
    let digits_at = base
        .rfind(|c: char| !c.is_ascii_digit())
        .map_or(0, |i| i + 1);
    match base[digits_at..].parse() {
        Ok(depth @ (9 | 10 | 12 | 14 | 16)) => Some(depth),
        _ => None,
    }
}

/// Maps pix_fmt names to the sampling notation.
fn chroma_subsampling(pix_fmt: &str) -> &'static str {
    let base = pix_fmt.strip_suffix("le").unwrap_or(pix_fmt);
    let base = base.strip_suffix("be").unwrap_or(base);
    match base.trim_end_matches(|c: char| c.is_ascii_digit()) {
        "yuv420p" => "4:2:0",
        "yuv422p" => "4:2:2",
        "yuv444p" => "4:4:4",
        _ => "",
    }
}

/// Renders "24/1" as "24.000".
fn format_frame_rate(v: &str) -> String {
    match parse_fps(v) {
        0.0 => String::new(),
        fps => format!("{fps:.3}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from the Go implementation (buildVideoExtra) for the same
    // streams; the Rust output must match byte for byte.
    const GOLDEN_HDR: &str = r#"{"ma:bitDepth":"10","ma:chromaLocation":"left","ma:chromaSubsampling":"4:2:0","ma:codedHeight":"2160","ma:codedWidth":"3840","ma:colorPrimaries":"bt2020","ma:colorRange":"tv","ma:colorSpace":"bt2020nc","ma:colorTrc":"smpte2084","ma:frameRate":"23.976","ma:hasScalingMatrix":"0","ma:height":"2160","ma:level":"120","ma:profile":"main 10","ma:refFrames":"1","ma:scanType":"progressive","ma:width":"3840","url":"ma%3AbitDepth=10&ma%3AchromaLocation=left&ma%3AchromaSubsampling=4%3A2%3A0&ma%3AcodedHeight=2160&ma%3AcodedWidth=3840&ma%3AcolorPrimaries=bt2020&ma%3AcolorRange=tv&ma%3AcolorSpace=bt2020nc&ma%3AcolorTrc=smpte2084&ma%3AframeRate=23.976&ma%3AhasScalingMatrix=0&ma%3Aheight=2160&ma%3Alevel=120&ma%3Aprofile=main+10&ma%3ArefFrames=1&ma%3AscanType=progressive&ma%3Awidth=3840"}"#;
    const GOLDEN_SDR: &str = r#"{"ma:bitDepth":"8","ma:chromaSubsampling":"4:2:0","ma:codedHeight":"720","ma:codedWidth":"1280","ma:frameRate":"24.000","ma:hasScalingMatrix":"0","ma:height":"720","ma:profile":"high","ma:scanType":"progressive","ma:width":"1280","url":"ma%3AbitDepth=8&ma%3AchromaSubsampling=4%3A2%3A0&ma%3AcodedHeight=720&ma%3AcodedWidth=1280&ma%3AframeRate=24.000&ma%3AhasScalingMatrix=0&ma%3Aheight=720&ma%3Aprofile=high&ma%3AscanType=progressive&ma%3Awidth=1280"}"#;
    const GOLDEN_ODD: &str = r#"{"ma:bitDepth":"8","ma:chromaLocation":"top left","ma:chromaSubsampling":"4:2:2","ma:codedHeight":"1080","ma:codedWidth":"1920","ma:frameRate":"29.970","ma:hasScalingMatrix":"0","ma:height":"1080","ma:level":"-99","ma:profile":"weird \"q\" \\ *~ ü\tx","ma:refFrames":"4","ma:scanType":"progressive","ma:width":"1920","url":"ma%3AbitDepth=8&ma%3AchromaLocation=top+left&ma%3AchromaSubsampling=4%3A2%3A2&ma%3AcodedHeight=1080&ma%3AcodedWidth=1920&ma%3AframeRate=29.970&ma%3AhasScalingMatrix=0&ma%3Aheight=1080&ma%3Alevel=-99&ma%3Aprofile=weird+%22q%22+%5C+%2A~+%C3%BC%09x&ma%3ArefFrames=4&ma%3AscanType=progressive&ma%3Awidth=1920"}"#;
    const GOLDEN_EMPTY: &str = r#"{"ma:hasScalingMatrix":"0","ma:scanType":"progressive","url":"ma%3AhasScalingMatrix=0&ma%3AscanType=progressive"}"#;

    fn hdr() -> Stream {
        Stream {
            width: 3840,
            height: 2160,
            coded_width: 3840,
            coded_height: 2160,
            profile: "Main 10".into(),
            pix_fmt: "yuv420p10le".into(),
            color_range: "tv".into(),
            color_primaries: "bt2020".into(),
            color_space: "bt2020nc".into(),
            color_transfer: "smpte2084".into(),
            chroma_location: "left".into(),
            bit_depth: "10".into(),
            level: 120,
            refs: 1,
            frame_rate: "24000/1001".into(),
            ..Default::default()
        }
    }

    #[test]
    fn matches_go_output() {
        let sdr = Stream {
            pix_fmt: "yuv420p".into(),
            profile: "High".into(),
            width: 1280,
            height: 720,
            frame_rate: "24/1".into(),
            ..Default::default()
        };
        // escaping corner cases: quotes, backslash, '*', '~', non-ASCII,
        // tab and a negative level
        let odd = Stream {
            width: 1920,
            height: 1080,
            profile: "Weird \"q\" \\ *~ \u{fc}\tx".into(),
            pix_fmt: "yuv422p".into(),
            chroma_location: "top left".into(),
            bit_depth: "N/A".into(),
            level: -99,
            refs: 4,
            frame_rate: "30000/1001".into(),
            ..Default::default()
        };
        assert_eq!(build_video_extra(&hdr()), GOLDEN_HDR);
        assert_eq!(build_video_extra(&sdr), GOLDEN_SDR);
        assert_eq!(build_video_extra(&odd), GOLDEN_ODD);
        assert_eq!(build_video_extra(&Stream::default()), GOLDEN_EMPTY);
    }

    #[test]
    fn output_is_valid_json() {
        let v: serde_json::Value = serde_json::from_str(&build_video_extra(&hdr())).unwrap();
        assert_eq!(v["ma:profile"], "main 10");
        assert!(v["url"].as_str().unwrap().starts_with("ma%3AbitDepth=10&"));
    }

    #[test]
    fn query_escape_like_go() {
        assert_eq!(query_escape("a b*~-_.:/%"), "a+b%2A~-_.%3A%2F%25");
    }

    #[test]
    fn bit_depth_fallbacks() {
        let depth = |pix_fmt: &str, raw: &str| {
            video_bit_depth(&Stream {
                pix_fmt: pix_fmt.into(),
                bit_depth: raw.into(),
                ..Default::default()
            })
        };
        // pix_fmt values as ffprobe reports them
        assert_eq!(depth("yuv420p10le", ""), "10");
        assert_eq!(depth("yuv422p12be", "N/A"), "12");
        assert_eq!(depth("yuv444p16be", "N/A"), "16");
        assert_eq!(depth("gray16be", "N/A"), "16");
        assert_eq!(depth("p010le", ""), "10");
        assert_eq!(depth("yuv420p9le", ""), "9");
        // digits that are part of the name, not a depth
        assert_eq!(depth("nv12", "N/A"), "8");
        assert_eq!(depth("nv16", ""), "8");
        assert_eq!(depth("rgb555le", ""), "8");
        assert_eq!(depth("yuv420p", "N/A"), "8");
        assert_eq!(depth("", ""), "");
        // bits_per_raw_sample wins
        assert_eq!(depth("yuv420p", "12"), "12");
    }

    #[test]
    fn chroma() {
        assert_eq!(chroma_subsampling("yuv420p10le"), "4:2:0");
        assert_eq!(chroma_subsampling("yuv422p12be"), "4:2:2");
        assert_eq!(chroma_subsampling("yuv444p"), "4:4:4");
        assert_eq!(chroma_subsampling("nv12"), "");
    }
}

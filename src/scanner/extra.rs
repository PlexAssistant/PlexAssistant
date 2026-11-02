//! extra_data the way Plex's analysis writes it: a JSON object of ma:/pv:
//! fields in key order plus a url-encoded copy. PMS derives the client's
//! displayTitle ("2160p HDR10", ...) and direct-play decisions from them.
//! Field rules are measured in tests/fixtures/plex-1.43.5.11079.

use std::collections::BTreeMap;
use std::fmt::Write;

use super::parse::parse_fps;
use super::probe::Stream;

/// One extra_data object under construction; empty values are left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Extra(BTreeMap<&'static str, String>);

impl Extra {
    pub fn set(&mut self, key: &'static str, value: impl Into<String>) {
        let value = value.into();
        if !value.is_empty() {
            self.0.insert(key, value);
        }
    }

    /// Serializes to Plex's form, or "" (stored as NULL) when empty.
    pub fn encode(&self) -> String {
        if self.0.is_empty() {
            return String::new();
        }
        let mut json = String::from("{");
        let mut url = String::new();
        for (key, value) in &self.0 {
            if !url.is_empty() {
                json.push(',');
                url.push('&');
            }
            let _ = write!(json, "{}:{}", json_string(key), json_string(value));
            let _ = write!(url, "{}={}", url_escape(key), url_escape(value));
        }
        let _ = write!(json, ",\"url\":{}}}", json_string(&url));
        json
    }
}

/// The ma: fields of a video stream. container is Plex's name for it.
pub(super) fn video_extra(s: &Stream, container: &str) -> Extra {
    let mut e = Extra::default();
    let sar = s.sample_aspect_ratio.as_str();
    if !matches!(sar, "" | "1:1" | "0:1" | "N/A") {
        e.set("ma:anamorphic", "1");
        e.set("ma:pixelAspectRatio", sar);
    }
    e.set("ma:bitDepth", video_bit_depth(s));
    e.set("ma:chromaLocation", known(&s.chroma_location));
    e.set("ma:chromaSubsampling", chroma_subsampling(&s.pix_fmt));
    // Plex keeps the sample entry tag only where it decides playback:
    // hvc1 vs hev1 for Apple clients, the MPEG-4 ASP flavour (xvid, divx)
    if matches!(s.codec_name.as_str(), "hevc" | "mpeg4") {
        e.set("ma:codecID", fourcc(&s.codec_tag_string));
    }
    // H.264 codes whole 16x16 macroblocks: 1080p is 1088 lines coded
    let coded = |v: i64, fallback: i64| match non_zero(v, fallback) {
        v if s.codec_name == "h264" => (v + 15) / 16 * 16,
        v => v,
    };
    e.set("ma:codedHeight", positive(coded(s.coded_height, s.height)));
    e.set("ma:codedWidth", positive(coded(s.coded_width, s.width)));
    e.set("ma:colorPrimaries", known(&s.color_primaries));
    e.set("ma:colorRange", known(&s.color_range));
    e.set("ma:colorSpace", known(&s.color_space));
    e.set("ma:colorTrc", known(&s.color_transfer));
    e.set("ma:frameRate", format_frame_rate(&s.frame_rate));
    if s.codec_name == "h264" {
        e.set("ma:hasScalingMatrix", "0");
    }
    e.set("ma:height", positive(s.height));
    // -99 is ffprobe's "unknown level"
    if s.level > 0 {
        e.set("ma:level", s.level.to_string());
    }
    e.set("ma:profile", plex_profile(&s.profile));
    e.set("ma:refFrames", positive(s.refs));
    e.set("ma:scanType", scan_type(&s.field_order));
    e.set("ma:streamIdentifier", stream_identifier(s, container));
    e.set("ma:width", positive(s.width));
    e
}

/// The ma: fields of an audio stream. Plex's own ffmpeg reports no
/// channel layout for AAC (outside FLV, whose header carries it) and DTS,
/// and names a profile only for DTS and PCM.
pub(super) fn audio_extra(s: &Stream, container: &str) -> Extra {
    let mut e = Extra::default();
    let layout_known = match s.codec_name.as_str() {
        "aac" => container == "flv",
        "dts" => false,
        _ => true,
    };
    if layout_known {
        e.set("ma:audioChannelLayout", known(&s.channel_layout));
    }
    // lossless codecs only; lossy ones report none
    let depth = s.bit_depth.parse().map_or_else(|_| String::new(), positive);
    e.set("ma:bitDepth", depth);
    match s.codec_name.as_str() {
        "dts" => e.set("ma:profile", plex_profile(&s.profile)),
        pcm if pcm.starts_with("pcm_") => e.set("ma:profile", pcm),
        _ => {}
    }
    e.set("ma:samplingRate", known(&s.sample_rate));
    e.set("ma:streamIdentifier", stream_identifier(s, container));
    e
}

/// Subtitle streams only carry their container-level id.
pub(super) fn subtitle_extra(s: &Stream, container: &str) -> Extra {
    let mut e = Extra::default();
    e.set("ma:streamIdentifier", stream_identifier(s, container));
    e
}

/// The container-level id of a stream: ffprobe's hex id in decimal (MP4
/// track id, MPEG-TS PID, ...). Where ffprobe shows none, Plex's demuxers
/// still number ASF streams from 1 and AVI/Ogg streams by index, with 0
/// meaning none.
pub(super) fn stream_identifier(s: &Stream, container: &str) -> String {
    let id = s.id.strip_prefix("0x").map_or_else(
        || s.id.parse().ok(),
        |hex| i64::from_str_radix(hex, 16).ok(),
    );
    let id = id.or(match container {
        "asf" => Some(s.index + 1),
        "avi" | "ogg" => Some(s.index),
        _ => None,
    });
    id.map(positive).unwrap_or_default()
}

/// Lowercased, without the trailing " profile" ("Simple Profile" is
/// "simple", "Profile 0" stays "profile 0").
pub(super) fn plex_profile(profile: &str) -> String {
    let profile = known(profile).to_lowercase();
    match profile.strip_suffix(" profile") {
        Some(base) => base.to_owned(),
        None => profile,
    }
}

fn scan_type(field_order: &str) -> &'static str {
    match field_order {
        "progressive" => "progressive",
        "tt" | "bb" | "tb" | "bt" => "interlaced",
        _ => "",
    }
}

/// A printable sample entry fourcc, lowercased ("hvc1", "XVID" -> "xvid");
/// ffprobe renders binary tags as "[0][0][0][0]".
fn fourcc(tag: &str) -> String {
    if tag.len() == 4 && tag.bytes().all(|b| b.is_ascii_alphanumeric()) {
        tag.to_ascii_lowercase()
    } else {
        String::new()
    }
}

/// ffprobe's placeholder for a missing value reads as missing.
fn known(value: &str) -> &str {
    match value {
        "unknown" | "unspecified" | "N/A" => "",
        v => v,
    }
}

fn non_zero(v: i64, fallback: i64) -> i64 {
    if v == 0 { fallback } else { v }
}

fn positive(v: i64) -> String {
    if v > 0 { v.to_string() } else { String::new() }
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).expect("strings always serialize")
}

/// Plex's percent-encoding: only ASCII letters, digits, '-' and '_' stay
/// literal; everything else, '.' and space included, is %XX.
fn url_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' => out.push(b as char),
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
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

    #[test]
    fn encodes_like_plex() {
        let mut e = Extra::default();
        e.set("ma:width", "1920");
        e.set("ma:frameRate", "23.976");
        e.set("ma:profile", "constrained baseline");
        e.set("ma:empty", "");
        // keys sorted, empty values dropped, '.' and ' ' percent-encoded
        assert_eq!(
            e.encode(),
            r#"{"ma:frameRate":"23.976","ma:profile":"constrained baseline","ma:width":"1920","url":"ma%3AframeRate=23%2E976&ma%3Aprofile=constrained%20baseline&ma%3Awidth=1920"}"#
        );
        assert_eq!(Extra::default().encode(), "");
    }

    #[test]
    fn url_escape_like_plex() {
        // values as Plex encoded them in tests/fixtures
        for (raw, want) in [
            ("5.1(side)", "5%2E1%28side%29"),
            ("7.1(wide-side)", "7%2E1%28wide-side%29"),
            ("pcm_s24le", "pcm_s24le"),
            ("4:2:0", "4%3A2%3A0"),
            (r#"{"Chapters":{}}"#, "%7B%22Chapters%22%3A%7B%7D%7D"),
        ] {
            assert_eq!(url_escape(raw), want);
        }
    }

    #[test]
    fn identifiers() {
        let id = |id: &str, index: i64, container: &str| {
            let s = Stream {
                id: id.into(),
                index,
                ..Default::default()
            };
            stream_identifier(&s, container)
        };
        assert_eq!(id("0x1", 0, "mp4"), "1");
        assert_eq!(id("0x1011", 0, "mpegts"), "4113");
        assert_eq!(id("", 0, "mkv"), "");
        assert_eq!(id("", 0, "asf"), "1");
        assert_eq!(id("", 1, "avi"), "1");
        assert_eq!(id("", 0, "avi"), "");
    }

    #[test]
    fn audio_fields() {
        let stream = |codec: &str, layout: &str, depth: &str, profile: &str| Stream {
            codec_name: codec.into(),
            channel_layout: layout.into(),
            bit_depth: depth.into(),
            profile: profile.into(),
            sample_rate: "48000".into(),
            ..Default::default()
        };
        let keys = |s: &Stream, container: &str| -> Vec<&'static str> {
            audio_extra(s, container).0.into_keys().collect()
        };
        assert_eq!(
            keys(&stream("aac", "stereo", "", "LC"), "mkv"),
            ["ma:samplingRate"]
        );
        assert_eq!(
            keys(&stream("aac", "stereo", "", "LC"), "flv"),
            ["ma:audioChannelLayout", "ma:samplingRate"]
        );
        let flac = audio_extra(&stream("flac", "mono", "16", ""), "mkv");
        assert_eq!(flac.0["ma:bitDepth"], "16");
        let dts = audio_extra(&stream("dts", "5.1(side)", "0", "DTS-HD MA"), "mkv");
        assert_eq!(dts.0["ma:profile"], "dts-hd ma");
        assert!(!dts.0.contains_key("ma:audioChannelLayout"));
        assert!(!dts.0.contains_key("ma:bitDepth"));
        let pcm = audio_extra(&stream("pcm_s24le", "mono", "24", ""), "mov");
        assert_eq!(pcm.0["ma:profile"], "pcm_s24le");
    }

    #[test]
    fn profiles() {
        assert_eq!(plex_profile("Simple Profile"), "simple");
        assert_eq!(plex_profile("Profile 0"), "profile 0");
        assert_eq!(plex_profile("Main 10"), "main 10");
        assert_eq!(plex_profile("unknown"), "");
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

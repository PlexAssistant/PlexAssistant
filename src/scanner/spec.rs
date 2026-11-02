use crate::linkres::Link;
use crate::plexdb::{Part, StreamRow, WriteSpec};

use super::extra::build_video_extra;
use super::lang::normalize_lang;
use super::parse::{atoi64, parse_fps, parse_ratio};
use super::probe::{ProbeResult, duration_ms};

/// Maps a probe result onto the row shapes the official scanner writes.
pub fn build_spec(part: &Part, probe: &ProbeResult, link: &Link) -> WriteSpec {
    let format = &probe.format;
    let matroska = format.format_name.starts_with("matroska");
    let mut spec = WriteSpec {
        media_item_id: part.media_item_id,
        part_id: part.id,
        duration_ms: duration_ms(&format.duration),
        bitrate: atoi64(&format.bit_rate),
        container: map_container(&format.format_name, &format.tags.major_brand),
        size: match atoi64(&format.size) {
            0 => link.size,
            size => size,
        },
        ..Default::default()
    };

    for s in &probe.streams {
        // data tracks (MP4 chapter text) and attachments (MKV fonts) are
        // not media streams; Plex stores no row for them
        let type_id = stream_type_id(&s.codec_type);
        if type_id == 0 {
            continue;
        }
        let mut row = StreamRow {
            type_id,
            codec: plex_codec(&s.codec_name),
            language: normalize_lang(&s.tags.language, matroska),
            index: s.index,
            channels: s.channels,
            bitrate: atoi64(&s.bit_rate),
            default: s.disposition.default != 0,
            forced: s.disposition.forced != 0,
            extra_data: String::new(),
        };
        if type_id == 1 {
            // the client's displayTitle ("2160p HDR10", ...) is derived
            // from the ma: fields in extra_data
            row.extra_data = build_video_extra(s);
            if spec.video_codec.is_empty() {
                spec.video_codec = row.codec.clone();
                spec.width = s.width;
                spec.height = s.height;
                spec.aspect_ratio = match parse_ratio(&s.display_aspect_ratio) {
                    0.0 if s.height > 0 => s.width as f64 / s.height as f64,
                    ratio => ratio,
                };
                spec.fps = parse_fps(&s.frame_rate);
                spec.color_trc = s.color_transfer.clone();
            }
        }
        spec.streams.push(row);
    }

    // the item's audio is the stream with the most channels, the first on
    // a tie: Plex picks 5.1 AC3 over a default stereo AAC
    if let Some(main) = spec
        .streams
        .iter()
        .filter(|r| r.type_id == 2)
        .reduce(|best, r| if r.channels > best.channels { r } else { best })
    {
        spec.audio_codec = main.codec.clone();
        spec.audio_channels = main.channels;
    }
    // a video stream without its own bitrate gets what the container's
    // leaves after the other streams' known bitrates
    let others: i64 = spec
        .streams
        .iter()
        .filter(|r| r.type_id != 1)
        .map(|r| r.bitrate)
        .sum();
    let rest = (spec.bitrate - others).max(0);
    for row in spec
        .streams
        .iter_mut()
        .filter(|r| r.type_id == 1 && r.bitrate == 0)
    {
        row.bitrate = rest;
    }
    spec
}

fn stream_type_id(codec_type: &str) -> i64 {
    match codec_type {
        "video" => 1,
        "audio" => 2,
        "subtitle" => 3,
        _ => 0,
    }
}

/// Plex's name for ffprobe's container, as measured in
/// tests/fixtures/plex-1.43.5.11079: Matroska and WebM are "mkv", the
/// ISO BMFF family is "mov" for QuickTime brands and "mp4" otherwise
/// (m4v included), and everything else keeps ffprobe's first name
/// ("mpegts" also for m2ts, "mpeg", "avi", "asf", "flv", "ogg").
fn map_container(format_name: &str, major_brand: &str) -> String {
    let first = format_name.split(',').next().unwrap_or_default().trim();
    match first {
        "matroska" | "webm" => "mkv",
        "mov" if major_brand.trim() == "qt" => "mov",
        "mov" => "mp4",
        other => other,
    }
    .to_owned()
}

/// Plex's name for ffprobe's codec where they differ; measured in
/// tests/fixtures/plex-1.43.5.11079.
fn plex_codec(codec_name: &str) -> String {
    match codec_name {
        "subrip" => "srt",
        "dts" => "dca",
        "hdmv_pgs_subtitle" => "pgs",
        "dvd_subtitle" => "vobsub",
        pcm if pcm.starts_with("pcm_") => "pcm",
        other => other,
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::probe::parse_probe;

    #[test]
    fn build() {
        let part = Part {
            id: 70,
            media_item_id: 7,
            file: "/data/a.strm".into(),
            size: 35,
            duration: 0,
        };
        let probe = parse_probe(
            br#"{
              "streams": [
                {"index": 0, "codec_name": "h264", "codec_type": "video",
                 "width": 1280, "height": 720, "bit_rate": "612000",
                 "avg_frame_rate": "24/1", "display_aspect_ratio": "16:9"},
                {"index": 1, "codec_name": "aac", "codec_type": "audio",
                 "channels": 2, "sample_rate": "48000", "bit_rate": "96000",
                 "tags": {"language": "und"}},
                {"index": 2, "codec_name": "subrip", "codec_type": "subtitle",
                 "tags": {"language": "chi"}}
              ],
              "format": {
                "format_name": "matroska,webm", "duration": "20.016000",
                "size": "3757806", "bit_rate": "1501731"
              }}"#,
        )
        .unwrap();
        let link = Link {
            url: "http://x/dl".into(),
            ..Default::default()
        };

        let spec = build_spec(&part, &probe, &link);
        assert_eq!((spec.media_item_id, spec.part_id), (7, 70));
        assert_eq!(
            (spec.duration_ms, spec.size, spec.bitrate),
            (20016, 3757806, 1501731)
        );
        assert_eq!(
            (
                spec.container.as_str(),
                spec.video_codec.as_str(),
                spec.audio_codec.as_str()
            ),
            ("mkv", "h264", "aac")
        );
        assert_eq!((spec.width, spec.height), (1280, 720));
        // 16:9 parsed, not the width/height fallback
        assert!(
            (1.7777..1.7778).contains(&spec.aspect_ratio),
            "{}",
            spec.aspect_ratio
        );
        assert_eq!(spec.fps, 24.0);
        assert_eq!(spec.audio_channels, 2);
        assert_eq!(
            spec.streams.iter().map(|s| s.type_id).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(spec.streams[2].language, "zh");
        // und is stored as no language
        assert_eq!(spec.streams[1].language, "");
        // a present stream bitrate is kept (fallback only fires when missing)
        assert_eq!(spec.streams[0].bitrate, 612000);
        assert!(spec.streams[0].extra_data.contains(r#""ma:width":"1280""#));
        assert!(spec.streams[1].extra_data.is_empty());
    }

    #[test]
    fn fallbacks() {
        let part = Part {
            id: 1,
            media_item_id: 1,
            file: "/data/a.strm".into(),
            size: 0,
            duration: 0,
        };
        let probe = parse_probe(
            br#"{
              "streams": [
                {"index": 0, "codec_name": "hevc", "codec_type": "video",
                 "width": 3840, "height": 2160, "avg_frame_rate": "24000/1001"}
              ],
              "format": {"format_name": "mpegts", "duration": "10.0"}}"#,
        )
        .unwrap();
        let link = Link {
            url: "x".into(),
            size: 12345,
            ..Default::default()
        };
        // no format size/bitrate: fall back to the resolved link size
        let spec = build_spec(&part, &probe, &link);
        assert_eq!(spec.size, 12345);
        assert_eq!(spec.bitrate, 0);
        assert_eq!(spec.streams[0].bitrate, 0);
        assert_eq!(spec.container, "mpegts");
        // no display_aspect_ratio: width/height fallback
        assert!(
            (1.777..1.778).contains(&spec.aspect_ratio),
            "{}",
            spec.aspect_ratio
        );
        assert_eq!((spec.fps * 1000.0) as i64, 23976);
    }

    #[test]
    fn first_video_and_audio_win() {
        let probe = parse_probe(
            br#"{"streams": [
                {"index": 0, "codec_name": "h264", "codec_type": "video", "width": 1920, "height": 1080},
                {"index": 1, "codec_name": "mjpeg", "codec_type": "video", "width": 320, "height": 240},
                {"index": 2, "codec_name": "eac3", "codec_type": "audio", "channels": 6},
                {"index": 3, "codec_name": "aac", "codec_type": "audio", "channels": 2}
              ]}"#,
        )
        .unwrap();
        let part = Part {
            id: 1,
            media_item_id: 1,
            file: String::new(),
            size: 0,
            duration: 0,
        };
        let spec = build_spec(&part, &probe, &Link::default());
        assert_eq!((spec.video_codec.as_str(), spec.width), ("h264", 1920));
        assert_eq!(
            (spec.audio_codec.as_str(), spec.audio_channels),
            ("eac3", 6)
        );
        assert_eq!(spec.streams.len(), 4);
    }

    #[test]
    fn skips_data_and_attachment_streams() {
        let probe = parse_probe(
            br#"{"streams": [
                {"index": 0, "codec_name": "h264", "codec_type": "video"},
                {"index": 1, "codec_name": "bin_data", "codec_type": "data"},
                {"index": 2, "codec_name": "ttf", "codec_type": "attachment"},
                {"index": 3, "codec_name": "aac", "codec_type": "audio"}
              ]}"#,
        )
        .unwrap();
        let part = Part {
            id: 1,
            media_item_id: 1,
            file: String::new(),
            size: 0,
            duration: 0,
        };
        let spec = build_spec(&part, &probe, &Link::default());
        assert_eq!(
            spec.streams.iter().map(|s| s.index).collect::<Vec<_>>(),
            [0, 3]
        );
    }

    #[test]
    fn containers() {
        for (format, brand, want) in [
            ("matroska,webm", "", "mkv"),
            ("mov,mp4,m4a,3gp,3g2,mj2", "qt  ", "mov"),
            ("mov,mp4,m4a,3gp,3g2,mj2", "isom", "mp4"),
            ("mov,mp4,m4a,3gp,3g2,mj2", "M4V ", "mp4"),
            ("mpegts", "", "mpegts"),
            ("mpeg", "", "mpeg"),
            ("avi", "", "avi"),
            ("asf", "", "asf"),
            ("flv", "", "flv"),
            ("ogg", "", "ogg"),
            ("", "", ""),
        ] {
            assert_eq!(map_container(format, brand), want, "{format} {brand:?}");
        }
    }

    #[test]
    fn codecs() {
        for (ffprobe, want) in [
            ("subrip", "srt"),
            ("dts", "dca"),
            ("hdmv_pgs_subtitle", "pgs"),
            ("dvd_subtitle", "vobsub"),
            ("pcm_s24le", "pcm"),
            ("pcm_s16le", "pcm"),
            ("h264", "h264"),
            ("mov_text", "mov_text"),
            ("truehd", "truehd"),
        ] {
            assert_eq!(plex_codec(ffprobe), want);
        }
    }
}

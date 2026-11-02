//! Compares our analysis with what a stock Plex wrote for the same files.
//!
//! Each fixture under tests/fixtures/plex-*/ holds the ffprobe output for
//! one media file plus the media_items, media_parts and media_streams rows
//! the official scanner stored for it (see scripts/plex-truth.sh). The
//! test feeds the ffprobe output through build_spec and reports every
//! column that differs.

use std::fs;
use std::path::PathBuf;

use plex_assistant::linkres::Link;
use plex_assistant::plexdb::{Part, WriteSpec};
use plex_assistant::scanner::{ProbeResult, build_spec};
use serde_json::Value;

struct Fixture {
    name: String,
    probe: ProbeResult,
    item: Value,
    part: Value,
    streams: Vec<Value>,
}

fn fixtures() -> Vec<Fixture> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut out = Vec::new();
    for dir in fs::read_dir(&root).unwrap() {
        let dir = dir.unwrap().path();
        for file in fs::read_dir(&dir).unwrap() {
            let path = file.unwrap().path();
            let raw: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            out.push(Fixture {
                name: format!(
                    "{}/{}",
                    dir.file_name().unwrap().to_string_lossy(),
                    raw["file"].as_str().unwrap()
                ),
                probe: serde_json::from_value(raw["ffprobe"].clone()).unwrap(),
                item: raw["media_item"].clone(),
                part: raw["media_part"].clone(),
                streams: raw["streams"].as_array().unwrap().clone(),
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    assert!(!out.is_empty(), "no fixtures under {}", root.display());
    out
}

fn analyze(f: &Fixture) -> WriteSpec {
    let part = Part {
        id: 1,
        media_item_id: 1,
        file: format!("/data/{}.strm", f.name),
        size: 0,
        duration: 0,
    };
    let link = Link {
        url: format!("http://cloud.example/dl/{}", f.name),
        ..Default::default()
    };
    build_spec(&part, &f.probe, &link)
}

/// Differences we accept, with the reason: (fixture suffix, column).
const KNOWN: &[(&str, &str, &str)] = &[
    (
        "h264_aac.flv",
        "stream 1.channels",
        "Plex reads the FLV header's stereo flag, ffprobe the AAC config (mono)",
    ),
    (
        "h264_aac.flv",
        "item.audio_channels",
        "same as stream 1.channels",
    ),
    (
        "h264_aac.flv",
        "stream 1.extra_data",
        "same: Plex records a stereo layout, ffprobe a mono one",
    ),
    (
        "h264_dts_truehd.mkv",
        "stream 1.bitrate",
        "Plex's ffmpeg reports no DTS bitrate, ffprobe 8.0.1 reads 1411200",
    ),
    (
        "h264_dts_truehd.mkv",
        "stream 0.bitrate",
        "so Plex leaves the DTS share in the video bitrate",
    ),
    (
        "hevc_hdr10.mp4",
        "stream 1.bitrate",
        "E-AC3 estimate differs by 119 bps",
    ),
];

/// Collects "where: ours != plex" lines.
struct Diff<'a> {
    fixture: &'a str,
    lines: Vec<String>,
}

impl Diff<'_> {
    fn push(&mut self, at: &str, line: String) {
        let known = KNOWN
            .iter()
            .any(|(file, col, _)| self.fixture.ends_with(file) && *col == at);
        if !known {
            self.lines.push(line);
        }
    }

    fn int(&mut self, at: &str, ours: i64, plex: &Value) {
        let plex = plex.as_i64().unwrap_or(0);
        if ours != plex {
            self.push(
                at,
                format!("{}: {at}: ours {ours} != plex {plex}", self.fixture),
            );
        }
    }

    fn float(&mut self, at: &str, ours: f64, plex: &Value) {
        let plex = plex.as_f64().unwrap_or(0.0);
        if (ours - plex).abs() > 1e-3 {
            self.push(
                at,
                format!("{}: {at}: ours {ours} != plex {plex}", self.fixture),
            );
        }
    }

    /// Compares two extra_data JSON objects, ignoring some keys.
    fn json(&mut self, at: &str, ours: &str, plex: &Value, ignore: &[&str]) {
        let parse = |s: &str| -> serde_json::Map<String, Value> {
            let mut map: serde_json::Map<String, Value> =
                serde_json::from_str(s).unwrap_or_default();
            map.retain(|k, _| !ignore.contains(&k.as_str()));
            map
        };
        let (ours, plex) = (parse(ours), parse(plex.as_str().unwrap_or("")));
        if ours != plex {
            self.push(
                at,
                format!("{}: {at}: ours {ours:?} != plex {plex:?}", self.fixture),
            );
        }
    }

    fn text(&mut self, at: &str, ours: &str, plex: &Value) {
        let plex = plex.as_str().unwrap_or("");
        if ours != plex {
            self.push(
                at,
                format!("{}: {at}: ours {ours:?} != plex {plex:?}", self.fixture),
            );
        }
    }
}

#[test]
fn matches_plex_analysis() {
    let mut lines = Vec::new();
    for f in fixtures() {
        let spec = analyze(&f);
        let mut d = Diff {
            fixture: &f.name,
            lines: Vec::new(),
        };
        let item = &f.item;
        d.int("item.width", spec.width, &item["width"]);
        d.int("item.height", spec.height, &item["height"]);
        d.int("item.duration", spec.duration_ms, &item["duration"]);
        d.int("item.bitrate", spec.bitrate, &item["bitrate"]);
        d.text("item.container", &spec.container, &item["container"]);
        d.text("item.video_codec", &spec.video_codec, &item["video_codec"]);
        d.text("item.audio_codec", &spec.audio_codec, &item["audio_codec"]);
        d.int(
            "item.audio_channels",
            spec.audio_channels,
            &item["audio_channels"],
        );
        d.float(
            "item.display_aspect_ratio",
            spec.aspect_ratio,
            &item["display_aspect_ratio"],
        );
        d.float(
            "item.frames_per_second",
            spec.fps,
            &item["frames_per_second"],
        );
        d.text("item.color_trc", &spec.color_trc, &item["color_trc"]);
        d.text("item.extra_data", &spec.item_extra, &item["extra_data"]);
        // Plex's ma:optimizedForStreaming (MP4/MOV) is not predictable from
        // ffprobe: 0 for the two files with PCM audio, 1 for the others,
        // with the moov box at the end in all of them
        d.json(
            "part.extra_data",
            &spec.part_extra,
            &f.part["extra_data"],
            &["url", "ma:optimizedForStreaming"],
        );
        d.int("part.size", spec.size, &f.part["size"]);
        d.int("part.duration", spec.duration_ms, &f.part["duration"]);

        // pair rows by ffprobe index; Plex skips data tracks entirely
        for ours in &spec.streams {
            if !f
                .streams
                .iter()
                .any(|p| p["index"].as_i64() == Some(ours.index))
            {
                d.push(
                    "stream",
                    format!("{}: stream {} not in Plex", f.name, ours.index),
                );
            }
        }
        for plex in &f.streams {
            let Some(ours) = spec
                .streams
                .iter()
                .find(|s| plex["index"].as_i64() == Some(s.index))
            else {
                d.push(
                    "stream",
                    format!("{}: stream {} missing", f.name, plex["index"]),
                );
                continue;
            };
            let at = |col: &str| format!("stream {}.{col}", ours.index);
            d.int(&at("index"), ours.index, &plex["index"]);
            d.int(&at("stream_type_id"), ours.type_id, &plex["stream_type_id"]);
            d.text(&at("language"), &ours.language, &plex["language"]);
            d.text(&at("codec"), &ours.codec, &plex["codec"]);
            d.int(&at("default"), ours.default.into(), &plex["default"]);
            d.int(&at("forced"), ours.forced.into(), &plex["forced"]);
            d.int(&at("bitrate"), ours.bitrate, &plex["bitrate"]);
            d.text(&at("extra_data"), &ours.extra_data, &plex["extra_data"]);
            let channels = if plex["channels"].is_null() {
                0
            } else {
                ours.channels
            };
            d.int(&at("channels"), channels, &plex["channels"]);
        }
        lines.extend(d.lines);
    }
    assert!(
        lines.is_empty(),
        "{} difference(s):\n{}",
        lines.len(),
        lines.join("\n")
    );
}

//! The slice of Plex's library schema that the writer touches, for tests.
//! A test adds its own rows after it.

/// `schema_migrations` on a whitelisted version, and the media tables a
/// write fills.
pub(crate) const LIBRARY: &str = r#"
CREATE TABLE schema_migrations (version varchar(255) NOT NULL);
INSERT INTO schema_migrations VALUES ('500000000001.231');
CREATE TABLE metadata_items (id INTEGER PRIMARY KEY, duration INTEGER);
CREATE TABLE media_items (id INTEGER PRIMARY KEY, metadata_item_id INTEGER,
  library_section_id INTEGER,
  width INTEGER, height INTEGER, size INTEGER, duration INTEGER, bitrate INTEGER,
  container TEXT, video_codec TEXT, audio_codec TEXT, color_trc TEXT,
  display_aspect_ratio REAL, frames_per_second REAL, audio_channels INTEGER,
  display_offset INTEGER, media_analysis_version INTEGER, extra_data TEXT, updated_at INTEGER);
CREATE TABLE media_parts (id INTEGER PRIMARY KEY, media_item_id INTEGER,
  file TEXT, size INTEGER DEFAULT 0, duration INTEGER, extra_data TEXT, updated_at INTEGER);
CREATE TABLE media_streams (id INTEGER PRIMARY KEY AUTOINCREMENT,
  stream_type_id INTEGER, media_item_id INTEGER, media_part_id INTEGER,
  codec TEXT, language TEXT, "index" INTEGER, channels INTEGER, bitrate INTEGER,
  "default" INTEGER, forced INTEGER, extra_data TEXT, url TEXT,
  created_at INTEGER, updated_at INTEGER);
"#;

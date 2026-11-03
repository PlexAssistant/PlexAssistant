# Changelog

## [0.2.2] — 2026-11-03

### Fixed

- Visual credits detection ran ffmpeg twice per file: a first pass
  whose output was discarded, followed by the real padded pass. For
  network strm targets that doubled the download and decode cost of
  every credits scan.

### Changed

- ffprobe now passes `-rw_timeout 50000000` (a 50-second read-stall
  bound): a wedged CDN connection fails in 50 seconds instead of
  hanging until the outer two-minute process timeout.
- ffprobe's protocol whitelist gains `crypto` and `httpproxy` (some
  CDNs with signed query strings fail the handshake without
  `crypto`).
- Log hygiene: URLs echoed by ffmpeg/ffprobe errors and stderr tails
  are printed with their query strings stripped, so CDN tokens no
  longer reach the scanner logs.
- The Plex schema gate now matches the `schema_migrations` major
  version only: Plex bugfix migrations (e.g. `.232`) no longer
  disable writes. A different major still refuses to write.

### Security

- The runtime daemon's unix socket is created with explicit `0o660`
  permissions instead of inheriting the service umask.

### Performance

- The scanner reuses one daemon client per process across item
  resolutions instead of rebuilding the HTTP client per item.
- Link cache and dead-link ledger statements go through SQLite's
  prepared-statement cache.

## [0.2.1] — 2026-11-03

### Added

- Intro detection via cross-season audio fingerprinting (chromaprint):
  fingerprints each episode's head, cross-compares within a season,
  writes the consensus intro marker. Handles intros that start at
  different offsets per episode (cold opens).
- Visual credits detection for non-black backgrounds: when the
  black-window heuristic comes up empty (text overlays over live
  picture), a second pass analyzes edge density at 0.5 fps and finds
  the longest sustained text run.
- Dead-link self-healing within scan rounds: a probe that hits an
  upstream 4xx gets one deduplicated refresh per target per round;
  persistently dead targets back off instead of being re-probed.
- Cloud drive protection: per-domain circuit breaker (3 failures,
  3-minute cooldown doubling to 30 minutes), dead-link ledger
  (1/2/4/7-day backoff), and hourly scan quota per domain.
- s6 service integration: the scanner/transcoder symlinks and the
  link-cache daemon self-heal on container restart.
- HDR badge support: `color_trc` written from the video stream's
  transfer characteristics.
- Full video stream `extra_data` set (profile, bit depth, chroma,
  color space, frame rate, scan type) so clients render stream
  labels instead of "Unknown".
- `assistant intro --section N` command for intro detection.
- Dev tooling: `strmock` (fake cloud drive with signed links and
  fault injection), `plex-truth.sh` (fixture capture),
  `bin-swap.sh` (local dev helper).

### Changed

- Complete rewrite from Go to Rust (single static binary, musl,
  zero host toolchain for cross-compilation).
- Credits detection scans only the last 10 minutes at 0.5 fps /
  320px instead of walking the whole file (~40× less to decode for
  an hour-long 4K episode).
- Stream languages stored as ISO 639-1 two-letter codes ("eng" →
  "en", "chi" → "zh"), matching what Plex itself writes.

### Fixed

- Video streams no longer show "Unknown (HEVC)" in the client —
  the profile and display title are derived from `extra_data`.
- NULL language and video bitrate values that crashed Plex's SQL
  layer on full metadata views.
- Transcoder crash on UA-bound cloud links (115 CDN): the wrapper
  now injects the resolving user agent before every swapped input.
- Orphaned transcode sessions blocking the concurrency limit after
  an EAE audio decoder timeout.

## [0.2.0] — 2026-07-23

### Added

- Credits markers (heuristic black-window detection).
- Install-time scan preferences (FSEvent).
- s6 boot service for the assistant daemon.
- Self-build Docker image (multi-stage, linuxserver/plex base).

## [0.1.0] — 2026-07-17

### Added

- Initial release: scanner/transcoder takeover, STRM analysis with
  ffprobe, direct SQLite writes, persistent link cache, cloud-drive
  protection (breaker, ledger, quota), install/verify/revert.

[0.2.1]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/PlexAssistant/PlexAssistant/releases/tag/v0.1.0

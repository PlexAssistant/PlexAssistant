# Changelog

## [Unreleased]

### Fixed

- A person's photo now moves with the name when the cast is rewritten.
  The Chinese tag row is created carrying the original row's
  `user_thumb_url`, and a row written without one is filled in.
- The localize rewrites now read every id in a `lockedFields` list. A lock
  behind another id was missed, so a scan could rewrite a sort title, a
  tag label or a cast name that was locked.

## [0.5.1] — 2026-11-04

### Added

- A scan writes Chinese sort titles, genre, mood and style labels, and
  cast names when `ZH_LOCALIZE` names them. Unset does nothing. `sort`,
  `tags` and `people` run independently.
- `sort` writes the stored title as pinyin initials. The display title
  stays. 重庆森林 sorts as `CQSL`. A title that mixes Han with kana is
  left as stored.
- `tags` rewrites genre, mood and style labels from a local table.
  `Action` becomes 动作, `Sad` becomes 悲伤. A label the table does not
  know stays.
- `people` rewrites director, writer and actor names from the Chinese
  credits of the item's TMDB id. A film uses its own id. An episode uses
  the show's id with the season and episode numbers. A show that stores
  its cast on itself uses the show credits. A name that comes back
  unchanged stays, and the role stays. A built-in v3 key is used unless
  `TMDB_API_KEY` is set. A request that does not come back skips that
  item; the rest of the scan still finishes.
- A field that was written is locked, so turning the switch off leaves
  it. `ZH_LOCALIZE_UNLOCK=1` drops those locks and leaves what was
  written. The next metadata refresh puts the source values back.

## [0.5.0] — 2026-11-04

### Fixed

- A successful resolve now stores the link, clears the failure and counts
  the cloud read in one commit. A crash between the three writes could
  leave a counted read without its link.
- An upstream error returned by the daemon has the query string stripped
  from any URL in it, so a signed link does not reach the caller.
- A failed probe no longer carries the signed input URL. ffprobe quotes
  it on stderr, and the query is stripped before the error leaves the
  probe.
- Intro detection after a section scan visited every season in the
  library, not only the seasons of that section.
- A scan's resolve and refresh were recorded as playback reads, so the
  daily cloud usage counted a scan's cache hits and file sizes.

### Changed

- The User-Agent on a resolved link and on playback is `Lavf/63.1.102`,
  the value ffprobe 9.0.2 sends on its own. A signed link is bound to
  the header that opened it, so both sides send the same value. Links
  signed under `PlexAssistant/0.1` are fetched again on the next scan
  or playback.
- The scanner, the runtime and `assistant intro` default to
  `/usr/local/bin/ffprobe` and `/usr/local/bin/ffmpeg`, the binaries the
  image installs. `ASSISTANT_FFPROBE` and `ASSISTANT_FFMPEG` still
  override them. `fpcalc` stays a bare command.
- A scan probe asks ffprobe only for the entries that reach a Plex row:
  the container and its brand, the stream fields `extra_data` is built
  from, and the chapter times. The duration, size and codecs are
  unchanged. On one 2160p episode the JSON went from 57064 bytes to
  33164.
- The preserved Plex binaries are named `.official` instead of `.real`.
  An existing install is renamed on the next boot.
- A credits run probes the file once. The black-window detector used to
  run its own ffprobe before the other detectors ran theirs.
- A scan opens the assistant cache once per item instead of once per
  lookup.
- `assistant intro` no longer takes `--ffprobe`, which it never used.

## [0.4.5] — 2026-11-03

### Fixed

- The scanner integration fixture was missing the columns a scan reads
  when it looks up a season, so `cargo test` failed after a successful
  analysis and the release image was not published.

## [0.4.4] — 2026-11-03

### Added

- An audio or subtitle choice is remembered per account and per show.
  Analysing a part again points the choice at the new stream rows, and a
  later episode of the same show gets that choice when the account has
  not picked one there yet. The stream ids from before the rewrite are
  kept. `assistant tracks restore` puts them back and forgets the choice.
- Cloud reads are recorded per domain and per day: how many reads, how
  many bytes, and how many of those reads opened past the start. A scan
  probe counts as one byte. Playback counts the size the resolve learned,
  and a `-ss` before the input counts as a seek. The quota and the
  breaker still count requests only, and playback still bypasses both.

## [0.4.3] — 2026-11-03

### Changed

- Credits frames are scaled to 320 wide, keeping the picture's height,
  and padded to the square only when they are scored. Padding inside the
  scale squeezed a 16:9 picture and left the window on the two-second
  sample grid. On one synthetic 16:9 clip the padded window was
  20.0–60.0s and the proportional window was 21.5–58.5s.

### Added

- Intro detection runs as part of a scan. A season with at least two strm
  episodes is fingerprinted after its items are analysed, and an episode
  whose intro is already booked is not fingerprinted again.
  `assistant intro` still runs the same detection on its own.

## [0.4.2] — 2026-11-03

### Added

- A strm part that could not be analysed is booked on its own: how many
  attempts, and whether the failure is permanent or should be left
  alone. A permanent part is not tried again. The link ledger still
  covers the URL.
- Opening an item's detail page analyses a strm that still has no media
  info. Plex only asks for analysis during a scan, so an item it already
  considers analysed stayed empty until the next one.
- A file is recognised by its duration and size, not by the strm URL.
  The same file under another path, another strm, or a re-added item is
  written from the stored analysis instead of being probed again. Intro
  and credits detection uses the same pair, so a resigned link does not
  run detection again.

## [0.4.1] — 2026-11-03

### Changed

- A library scan probes three items at once. The results are still written
  one item at a time, and resolving a link still goes through the daemon's
  quota.

## [0.4.0] — 2026-11-03

### Added

- Intro and credits detection keeps a ledger of what it wrote. A scan
  skips an item whose content has not changed, and a marker set by hand
  is not replaced. `assistant marker` lists the rows and can set, retry,
  or stop one.
- Credits detection reads the box around the text and uses it to move the
  start and end of a scrolling credit off the two-second sample grid. A
  credit that stands still is unchanged.

## [0.3.1] — 2026-11-03

### Fixed

- A resolve that cannot succeed is no longer retried on the dead-link
  backoff. An unparseable URL, or an upstream that answers 404 or 410,
  is skipped until a later success clears it. A refusal, a stall, or a
  redirect loop still backs off for 1/2/4/7 days. A probe that fails
  for any other reason is booked the same way, instead of being
  re-probed on every later scan.

## [0.3.0] — 2026-11-03

### Added

- Credits detection can use the model Plex already stores
  (`model_v1.pb` under the scanners directory). A frame counts as
  text when enough of its score cells clear the threshold, and the
  marker is the longest such run of at least 15 seconds.
- `ASSISTANT_CREDITS` selects the detector: `auto` (default),
  `heuristic`, or `model`. `model` writes nothing when the stored
  file is missing or will not run.

### Changed

- The default credits chain is now black window, then the stored
  model, then the edge detector. Set `ASSISTANT_CREDITS=heuristic`
  to keep the 0.2.2 chain.

### Fixed

- Resolving a link through the daemon cache now checks the cached
  expiry: a link past its expiry gets one eager refresh instead of
  being handed to ffprobe as-is. Credits detection fired long after
  the scan that cached the link no longer fails silently on a stale
  signed URL.

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

[0.5.1]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.4.5...v0.5.0
[0.4.5]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.4.4...v0.4.5
[0.4.4]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.4.3...v0.4.4
[0.4.3]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.4.2...v0.4.3
[0.4.2]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.2.2...v0.3.0
[0.2.2]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/PlexAssistant/PlexAssistant/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/PlexAssistant/PlexAssistant/releases/tag/v0.1.0

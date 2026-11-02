# PlexAssistant

English | [正體中文](README.zh-TW.md)

Native STRM support for Plex: `.strm` items show real media info and
play, without getting your cloud-drive account rate-limited.

## How it works

Plex scans a `.strm` as a 35-byte text file: no duration, resolution
or audio tracks in the UI, and every playback re-resolves the URL
inside with whatever user agent it likes — the traffic pattern that
gets cloud-drive accounts throttled.

PlexAssistant wraps the two binaries Plex spawns for library work:

- **Scanner**: when Plex asks for an item's analysis, the assistant
  resolves the strm target, probes the direct link with ffprobe (the
  same user agent that resolved it; CDNs like 115 bind links to it)
  and writes duration, codecs, resolution, HDR badge and subtitle
  languages straight into the library database, shaped exactly like
  the official rows.
- **Transcoder**: at playback the `-i` strm path is swapped for a
  cached direct link before the original transcoder runs. A link that
  dies mid-playback is refreshed and the session retried once, in
  milliseconds.

Between them a resident daemon keeps a persistent link cache: each
target is resolved once and trusted until it fails. A dead link found
during a scan gets exactly one refresh per target per round; targets
that stay dead back off 1/2/4/7 days instead of being re-probed.
Scan-driven resolutions are bounded by a per-domain circuit breaker
and an hourly quota; playback always passes.

## Usage

Swap the image in your existing Plex deployment; nothing else changes:

```yaml
services:
  plex:
    image: ghcr.io/plexassistant/plexassistant:latest
```

```bash
docker compose up -d
```

Point a Plex library at your strm folder and the media info appears
on its own.

## Limitations

- No web UI.
- Mixed libraries work, but only strm items get the assistant's
  analysis.
- A few direct-play clients may bypass the server-side transcoder.
- Intro/credits markers are not implemented yet.

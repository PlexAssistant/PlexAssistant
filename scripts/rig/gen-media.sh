#!/usr/bin/env bash
# Generate test media (ffmpeg) and .strm samples pointing at strmock.
# Usage: gen-media.sh [base-url] [out-dir]
set -euo pipefail

BASE="${1:-http://strmock:8399}"
OUT="${2:-testdata}"
MEDIA="$OUT/media"
STRM="$OUT/strm"

mkdir -p "$MEDIA" "$STRM"

ff() { ffmpeg -y -hide_banner -loglevel error "$@"; }

# ep1: 720p, one stereo track
ff -f lavfi -i testsrc2=size=1280x720:rate=24:duration=20 \
   -f lavfi -i sine=frequency=440:duration=20 \
   -c:v libx264 -preset veryfast -crf 28 -pix_fmt yuv420p \
   -c:a aac -b:a 96k -shortest "$MEDIA/ep1.mkv"

# ep2: 1080p, two audio tracks
ff -f lavfi -i testsrc2=size=1920x1080:rate=24:duration=20 \
   -f lavfi -i sine=frequency=440:duration=20 \
   -f lavfi -i sine=frequency=660:duration=20 \
   -c:v libx264 -preset veryfast -crf 28 -pix_fmt yuv420p \
   -c:a aac -b:a 96k -map 0:v -map 1:a -map 2:a -shortest "$MEDIA/ep2.mkv"

# ep3: 480p SD
ff -f lavfi -i testsrc2=size=720x480:rate=24:duration=20 \
   -f lavfi -i sine=frequency=330:duration=20 \
   -c:v libx264 -preset veryfast -crf 28 -pix_fmt yuv420p \
   -c:a aac -b:a 96k -shortest "$MEDIA/ep3.mkv"

SHOW="STRM Test Show (2026)"
SEASON="$STRM/$SHOW/Season 01"
mkdir -p "$SEASON"

strm() { printf '%s/file/%s\n' "$BASE" "$2" > "$SEASON/$SHOW - $1.strm"; }

strm "S01E01 - normal"      ep1.mkv
strm "S01E02 - multi-audio" ep2.mkv
strm "S01E03 - slow"        ep3.mkv    # ctl: {"id":"ep3.mkv","behavior":"slow","bps":65536}
strm "S01E04 - expiring"    ep2.mkv    # ctl: {"id":"ep2.mkv","ttl":"10s"}
strm "S01E05 - dead"        missing.mkv

echo "media:  $MEDIA ($(du -sh "$MEDIA" | cut -f1))"
echo "strm:   $SEASON"
echo "base:   $BASE"

#!/usr/bin/env bash
# Capture ground truth from a stock Plex: generate a media set covering
# the codecs, containers, languages and flags we care about, let the
# official scanner analyze it, and write one fixture per file with the
# ffprobe output (from the image's own ffprobe) and the rows Plex wrote.
#
# Usage: plex-truth.sh [fixtures-root] [plex-image]
# Needs docker, ffmpeg (with x264/x265/svt-av1/vpx/theora/xvid), jq.
set -euo pipefail

mkdir -p "${1:-tests/fixtures}"
ROOT="$(cd "${1:-tests/fixtures}" && pwd)"
IMAGE="${2:-lscr.io/linuxserver/plex:latest}"
WORK="$(mktemp -d)"
NAME="plex-truth-$$"
DB="/config/Library/Application Support/Plex Media Server/Plug-in Support/Databases/com.plexapp.plugins.library.db"
cleanup() {
  # Plex's files may belong to a uid the host user cannot delete (rootless
  # podman), so empty the config from inside the container first
  docker exec "$NAME" rm -rf /config/Library >/dev/null 2>&1 || true
  docker rm -f "$NAME" >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

mkdir -p "$WORK/media" "$WORK/config"
cd "$WORK"

printf '1\n00:00:00,500 --> 00:00:02,000\nhello\n\n2\n00:00:02,500 --> 00:00:03,500\nworld\n' > sub.srt
ffmpeg -hide_banner -loglevel error -i sub.srt sub.ass
printf ';FFMETADATA1\n\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=1500\ntitle=Opening\n\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=1500\nEND=4000\ntitle=Main Part\n' > chapters.txt

ff() { ffmpeg -y -hide_banner -loglevel error "$@"; }
SIX='pan=5.1|c0=c0|c1=c0|c2=c0|c3=c0|c4=c0|c5=c0'
HD="-f lavfi -i testsrc2=size=1920x1080:rate=24000/1001:duration=4"
SD="-f lavfi -i testsrc2=size=1280x720:rate=24:duration=4"
TONE="-f lavfi -i sine=duration=4"

echo "generating media"
# shellcheck disable=SC2086
{
ff $HD $TONE -f lavfi -i sine=frequency=330:duration=4 -i sub.srt -i sub.srt \
  -filter_complex "[2:a]${SIX}[a6]" -map 0:v -map 1:a -map "[a6]" -map 3 -map 4 \
  -c:v libx264 -preset ultrafast -pix_fmt yuv420p -c:a:0 aac -c:a:1 ac3 -c:s srt \
  -metadata:s:a:0 language=en -metadata:s:a:1 language=jpn -metadata:s:s:0 language=chi -metadata:s:s:1 language=eng \
  -disposition:a:0 default -disposition:a:1 0 -disposition:s:0 default -disposition:s:1 forced media/h264_multi.mkv
ff -f lavfi -i testsrc2=size=3840x2160:rate=24:duration=2 -f lavfi -i sine=duration=2 -filter_complex "[1:a]${SIX}[a6]" -map 0:v -map "[a6]" \
  -c:v libx265 -preset ultrafast -x265-params 'log-level=error:hdr10=1:colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):max-cll=1000,400' \
  -pix_fmt yuv420p10le -tag:v hvc1 -c:a eac3 media/hevc_hdr10.mp4
ff -f lavfi -i testsrc2=size=1280x720:rate=30:duration=4 $TONE -c:v libvpx-vp9 -deadline realtime -cpu-used 8 -b:v 500k -c:a libopus -metadata:s:a:0 language=de media/vp9_opus.webm
ff -f lavfi -i testsrc2=size=1280x720:rate=25:duration=4 $TONE -c:v libsvtav1 -preset 12 -svtav1-params log-level=0 -pix_fmt yuv420p10le -c:a flac media/av1_flac.mkv
ff -f lavfi -i testsrc2=size=1920x1080:rate=30000/1001:duration=4 $TONE -vf interlace=scan=tff -c:v mpeg2video -flags +ilme+ildct -top 1 -b:v 8M -c:a mp2 media/mpeg2_interlaced.ts
ff -f lavfi -i testsrc2=size=720x480:rate=30000/1001:duration=4 $TONE -vf setsar=32/27 -c:v libxvid -q:v 4 -c:a libmp3lame -b:a 128k media/xvid_mp3.avi
ff -f lavfi -i testsrc2=size=1920x1080:rate=24:duration=2 -f lavfi -i sine=duration=2 -c:v prores_ks -profile:v 2 -pix_fmt yuv422p10le -c:a pcm_s24le media/prores_pcm.mov
ff $HD $TONE -filter_complex "[1:a]${SIX},asplit[a][b]" -map 0:v -map "[a]" -map "[b]" \
  -c:v libx264 -preset ultrafast -c:a:0 dca -c:a:1 truehd -strict -2 -metadata:s:a:0 language=fre -metadata:s:a:1 language=ger media/h264_dts_truehd.mkv
ff $SD $TONE -i sub.srt -filter_complex "[1:a]${SIX}[a6]" -map 0:v -map "[a6]" -map 2 \
  -c:v libx264 -preset ultrafast -c:a aac -c:s mov_text -metadata:s:s:0 language=fre media/h264_movtext.mp4
ff -f lavfi -i testsrc2=size=640x480:rate=25:duration=4 $TONE -c:v wmv2 -b:v 1M -c:a wmav2 media/wmv2_wma.wmv
ff $SD $TONE -i sub.ass -map 0:v -map 1:a -map 2 -c:v libx264 -preset ultrafast -c:a libvorbis -c:s ass media/h264_vorbis_ass.mkv
ff -f lavfi -i testsrc2=size=1280x720:rate=60:duration=4 $TONE -c:v libx265 -preset ultrafast -x265-params log-level=error -pix_fmt yuv420p -c:a alac media/hevc8_alac.mov
ff $SD $TONE -c:v mpeg2video -b:v 4M -c:a mp2 -f vob media/ps_mpeg2.mpg
ff $SD $TONE -c:v libx264 -preset ultrafast -c:a aac -f flv media/h264_aac.flv
ff $SD $TONE -c:v libx264 -preset ultrafast -c:a ac3 -mpegts_m2ts_mode 1 media/h264_ac3.m2ts
ff $SD $TONE -c:v libx264 -preset ultrafast -c:a aac -f ipod media/h264_aac.m4v
ff $SD $TONE -c:v libtheora -q:v 5 -c:a libvorbis media/theora_vorbis.ogv
ff $SD $TONE -i sub.srt -i sub.srt -map 0:v -map 1:a -map 2 -map 3 -c:v libx264 -preset ultrafast -c:a aac -c:s:0 ssa -c:s:1 webvtt media/subs_ssa_vtt.mkv
ff $SD $TONE -i chapters.txt -map 0:v -map 1:a -map_chapters 2 -c:v libx264 -preset ultrafast -c:a aac media/chapters.mkv
ff $SD $TONE -i chapters.txt -map 0:v -map 1:a -map_chapters 2 -c:v libx264 -preset ultrafast -c:a aac media/chapters.mp4
ff $SD $TONE -filter_complex "[1:a]asplit=5[a][b][c][d][e]" -map 0:v -map "[a]" -map "[b]" -map "[c]" -map "[d]" -map "[e]" \
  -c:v libx264 -preset ultrafast -c:a aac -metadata:s:a:0 language=fil -metadata:s:a:1 language=zho -metadata:s:a:2 language=por \
  -metadata:s:a:3 language=kor -metadata:s:a:4 language=und media/languages.mkv
ff $SD $TONE -filter_complex "[1:a]pan=7.1(wide-side)|c0=c0|c1=c0|c2=c0|c3=c0|c4=c0|c5=c0|c6=c0|c7=c0[a8]" -map 0:v -map "[a8]" \
  -c:v libx264 -preset ultrafast -c:a pcm_s16le media/pcm_widelayout.mov
}
COUNT=$(find media -type f | wc -l)

echo "starting $IMAGE"
docker run -d --name "$NAME" -e PUID="$(id -u)" -e PGID="$(id -g)" -e VERSION=docker \
  -v "$WORK/config:/config" -v "$WORK/media:/media:ro" "$IMAGE" >/dev/null
api() { docker exec "$NAME" curl -s "$@"; }
until api http://127.0.0.1:32400/identity | grep -q 'version=' && ! api http://127.0.0.1:32400/identity | grep -q startState; do sleep 2; done
VERSION=$(api http://127.0.0.1:32400/identity | sed -n 's/.*<MediaContainer.* version="\([^"]*\)".*/\1/p')
echo "plex $VERSION: analyzing $COUNT files"
api -X POST -o /dev/null 'http://127.0.0.1:32400/library/sections?name=Truth&type=movie&agent=tv.plex.agents.none&scanner=Plex%20Video%20Files&language=xn&location=%2Fmedia'
plexsql() { docker exec "$NAME" "/usr/lib/plexmediaserver/Plex SQLite" "$DB" "$@"; }
until [ "$(plexsql 'select count(distinct media_item_id) from media_streams;' 2>/dev/null || echo 0)" -ge "$COUNT" ]; do sleep 2; done
sleep 10 # let the last analysis commit
plexsql ".backup /tmp/truth.db"
docker cp "$NAME:/tmp/truth.db" truth.db
# probe with the ffprobe the release image ships, not the host's
docker exec "$NAME" sh -c 'apt-get update -qq >/dev/null && apt-get install -y -qq --no-install-recommends ffmpeg >/dev/null'

q() { docker exec "$NAME" "/usr/lib/plexmediaserver/Plex SQLite" -json /tmp/truth.db "$1"; }

OUT="$ROOT/plex-${VERSION%%-*}"
rm -rf "$OUT" && mkdir -p "$OUT"
for path in media/*; do
  f=$(basename "$path")
  docker exec "$NAME" ffprobe -v error -probesize 8388608 -show_format -show_streams -show_chapters -of json "/media/$f" \
    | jq 'del(.format.filename)' > probe.json
  q "select mi.container, mi.width, mi.height, mi.duration, mi.bitrate, mi.video_codec, mi.audio_codec,
       mi.display_aspect_ratio, mi.frames_per_second, mi.audio_channels, mi.color_trc, mi.extra_data
     from media_items mi join media_parts p on p.media_item_id = mi.id where p.file = '/media/$f'" > item.json
  q "select size, duration, extra_data from media_parts where file = '/media/$f'" > part.json
  q "select s.stream_type_id, s.\"index\", s.codec, s.language, s.channels, s.bitrate, s.\"default\", s.forced, s.url, s.extra_data
     from media_streams s join media_parts p on p.id = s.media_part_id where p.file = '/media/$f' order by s.\"index\"" > streams.json
  jq -n --arg file "$f" --arg plex "$VERSION" --slurpfile probe probe.json --slurpfile item item.json \
     --slurpfile part part.json --slurpfile streams streams.json \
     '{file: $file, plex: $plex, ffprobe: $probe[0], media_item: $item[0][0], media_part: $part[0][0], streams: $streams[0]}' \
     > "$OUT/${f}.json"
done
echo "wrote $COUNT fixtures to $OUT"

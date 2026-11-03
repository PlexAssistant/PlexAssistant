# syntax=docker/dockerfile:1
# Release image: build the assistant, bake it into linuxserver/plex
# with the s6 services. Works for any platform via buildx.
#
# Other targets:
#   dist         the static binary alone (just build-linux)
#   strmock    the test rig's fake cloud drive (compose.dev.yaml)
#   ffprobe-bin  the image's ffprobe alone (scripts/plex-truth.sh)
FROM rust:1.99-alpine AS build
RUN apk add --no-cache build-base
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
ARG TARGETPLATFORM
# musl static binaries, so they run in any container like the Go build did.
# Dependency layer built from stub mains: its cache key has no version
# string in it, so a release only recompiles the two project bins on top.
# target/ is cached per platform so multi-arch builds do not share it; the
# registry is shared, but locked, since cargo's own lock lives outside it.
RUN --mount=type=cache,id=cargo-registry,sharing=locked,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-target-${TARGETPLATFORM},target=/src/target \
    mkdir -p src/bin/strmock && \
    printf 'fn main() {}\n' | tee src/main.rs src/bin/strmock/main.rs >/dev/null && \
    cargo build --release --locked --bins && \
    rm -rf src
COPY src ./src
ARG VERSION=dev
ENV ASSISTANT_VERSION=${VERSION}
RUN --mount=type=cache,id=cargo-registry,sharing=locked,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-target-${TARGETPLATFORM},target=/src/target \
    cargo build --release --locked --bins && \
    mkdir -p /out && cp target/release/assistant target/release/strmock /out/

# Static ffprobe and ffmpeg. A distro package pulls hundreds of MB of
# unrelated libraries for these two binaries. n9.0.2 (946fcce07b6d),
# checksum-pinned. It carries CVE-2026-8461, CVE-2026-12706,
# CVE-2026-30997, CVE-2026-30998, CVE-2026-30999, CVE-2026-66038,
# CVE-2026-70629 and CVE-2026-70631, which 8.0.1 does not.
# https://ffmpeg.org/security.html
FROM alpine:3.24 AS ffprobe
RUN apk add --no-cache build-base nasm pkgconf openssl-dev openssl-libs-static zlib-dev zlib-static
ARG FFMPEG_VERSION=9.0.2
ARG FFMPEG_SHA256=6e374ed621e48faa40639307dff48ba6fe574a509977956d2cce9669b7cc27e9
WORKDIR /ffmpeg
RUN wget -qO ffmpeg.tar.gz "https://github.com/FFmpeg/FFmpeg/archive/refs/tags/n${FFMPEG_VERSION}.tar.gz" && \
    echo "${FFMPEG_SHA256}  ffmpeg.tar.gz" | sha256sum -c - && \
    tar xzf ffmpeg.tar.gz --strip-components=1 && rm ffmpeg.tar.gz
# swscale and swresample stay, or configure drops decoders that need them.
# OpenSSL 3 needs --enable-version3. -w hides the pinned source's warning
# flood. Demuxers and parsers stay open; decoders are the film set.
RUN ./configure \
        --pkg-config-flags=--static --extra-ldflags=-static \
        --extra-cflags=-w \
        --enable-version3 --disable-autodetect --enable-openssl --enable-zlib \
        --enable-swscale --enable-swresample \
        --disable-everything \
        --enable-muxer=null,wav,rawvideo \
        --enable-encoder=wrapped_avframe,pcm_s16le,rawvideo \
        --enable-filter=blackdetect,silencedetect,format,fps,scale,aresample,aformat,edgedetect,pad \
        --disable-doc --disable-debug --disable-avdevice \
        --enable-demuxers --enable-parsers --enable-bsfs \
        --enable-decoder='h264,hevc,av1,vp8,vp9,mpeg1video,mpeg2video,mpeg4,mjpeg,vc1,msmpeg4v1,msmpeg4v2,msmpeg4v3,h263,prores,ffv1,huffyuv,theora,dvvideo,rv10,rv20,rv30,rv40,cavs,aac,ac3,eac3,truehd,mlp,dca,flac,opus,vorbis,mp3,alac,wmav1,wmav2,ape,wavpack,pcm_s8,pcm_u8,pcm_s16le,pcm_s16be,pcm_s24le,pcm_s24be,pcm_s32le,pcm_s32be,pcm_f32le,pcm_f32be,pcm_f64le,pcm_f64be,pcm_alaw,pcm_mulaw,pcm_bluray,pcm_dvd' \
        --enable-protocol=file,pipe,data,crypto,tcp,tls,http,https,httpproxy && \
    make -j"$(nproc)" ffprobe ffmpeg && \
    strip ffprobe ffmpeg && mkdir -p /out && cp ffprobe ffmpeg /out/

FROM scratch AS ffprobe-bin
COPY --from=ffprobe /out/ffprobe /ffprobe

FROM scratch AS dist
ARG TARGETARCH
COPY --from=build /out/assistant /assistant-linux-${TARGETARCH}

FROM scratch AS strmock
COPY --from=build /out/strmock /strmock
EXPOSE 8399
ENTRYPOINT ["/strmock"]

FROM lscr.io/linuxserver/plex:latest
ARG VERSION=dev
ARG REVISION=""
ARG CREATED=""
# replace the base image's labels, which describe linuxserver's build
LABEL org.opencontainers.image.title="PlexAssistant" \
      org.opencontainers.image.description="Plex with native STRM support" \
      org.opencontainers.image.authors="PlexAssistant" \
      org.opencontainers.image.vendor="PlexAssistant" \
      org.opencontainers.image.url="https://github.com/PlexAssistant/PlexAssistant" \
      org.opencontainers.image.documentation="https://github.com/PlexAssistant/PlexAssistant" \
      org.opencontainers.image.source="https://github.com/PlexAssistant/PlexAssistant" \
      org.opencontainers.image.licenses="GPL-3.0" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}" \
      org.opencontainers.image.ref.name="${VERSION}" \
      org.opencontainers.image.created="${CREATED}"
COPY packaging/s6-overlay/ /etc/s6-overlay/
COPY --from=ffprobe /out/ffprobe /out/ffmpeg /usr/local/bin/
COPY --from=build /out/assistant /usr/local/bin/assistant
# fpcalc for the intro fingerprinting (tiny; pulls no ffmpeg of its own)
RUN apt-get update && \
    apt-get install -y --no-install-recommends libchromaprint-tools && \
    rm -rf /var/lib/apt/lists/*

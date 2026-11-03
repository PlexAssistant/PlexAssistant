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
COPY src ./src
ARG VERSION=dev
ARG TARGETPLATFORM
ENV ASSISTANT_VERSION=${VERSION}
# musl static binaries, so they run in any container like the Go build did.
# target/ is cached per platform so multi-arch builds do not share it; the
# registry is shared, but locked, since cargo's own lock lives outside it.
RUN --mount=type=cache,id=cargo-registry,sharing=locked,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-target-${TARGETPLATFORM},target=/src/target \
    cargo build --release --locked --bins && \
    mkdir -p /out && cp target/release/assistant target/release/strmock /out/

# ffprobe for the scanner, built static with only what probing needs:
# demuxers, parsers, decoders and the network protocols. Ubuntu's ffmpeg
# package would pull ~440 MB (LLVM, Mesa, ...) into the image for this
# one tool. The source is GitHub's archive of FFmpeg's n8.0.1 tag (commit
# 894da5ca7d74), pinned by checksum; 8.0.1 matches the ffprobe the
# fixtures in tests/fixtures were first captured with.
FROM alpine:3.24 AS ffprobe
RUN apk add --no-cache build-base nasm pkgconf openssl-dev openssl-libs-static zlib-dev zlib-static
ARG FFMPEG_VERSION=8.0.1
ARG FFMPEG_SHA256=679aa13a19415d5ddab91e580084e3ab20c963c8240001e5cbb955a97bdd81b1
WORKDIR /ffmpeg
RUN wget -qO ffmpeg.tar.gz "https://github.com/FFmpeg/FFmpeg/archive/refs/tags/n${FFMPEG_VERSION}.tar.gz" && \
    echo "${FFMPEG_SHA256}  ffmpeg.tar.gz" | sha256sum -c - && \
    tar xzf ffmpeg.tar.gz --strip-components=1 && rm ffmpeg.tar.gz
# swresample/swscale stay: decoders such as opus depend on them, and
# configure would silently drop those decoders without them. OpenSSL 3
# (Apache-2.0) needs --enable-version3, which suits this GPL-3.0 project.
# -w silences ffmpeg's flood of benign gcc warnings; the version
# is checksum-pinned, so there is nothing to read in them for us.
RUN ./configure \
        --pkg-config-flags=--static --extra-ldflags=-static \
        --extra-cflags=-w \
        --enable-version3 --disable-autodetect --enable-openssl --enable-zlib \
        --disable-everything \
        --enable-muxer=null --enable-encoder=wrapped_avframe --enable-filter=blackdetect,silencedetect,format,fps,scale \
        --disable-doc --disable-debug --disable-avdevice \
        --enable-demuxers --enable-parsers --enable-decoders --enable-bsfs \
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

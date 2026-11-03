# syntax=docker/dockerfile:1
# Release image: build the assistant, bake it into linuxserver/plex
# with the s6 services. Works for any platform via buildx.
#
# Other targets:
#   dist         the static binary alone (just build-linux)
#   strmock      the test rig's fake cloud drive (compose.dev.yaml)
#   ffprobe-bin  the image's ffprobe alone (scripts/truth/plex-truth.sh)
FROM rust:1.99-alpine AS build
RUN apk add --no-cache build-base
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
ARG TARGETPLATFORM
# Stub mains, so this layer's cache key has no version string in it and a
# release only recompiles the two bins. target/ is cached per platform:
# multi-arch builds cannot share it.
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

# Static ffprobe and ffmpeg, built and pinned in PlexAssistant/FFmpeg.
# n9.0.2 carries CVE-2026-8461, CVE-2026-12706, CVE-2026-30997,
# CVE-2026-30998, CVE-2026-30999, CVE-2026-66038, CVE-2026-70629
# and CVE-2026-70631, which 8.0.1 does not.
# https://ffmpeg.org/security.html
FROM ghcr.io/plexassistant/ffmpeg:v9.0.2 AS ffmpeg

FROM scratch AS ffprobe-bin
COPY --from=ffmpeg /out/ffprobe /ffprobe

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
COPY --from=ffmpeg /out/ffprobe /out/ffmpeg /usr/local/bin/
COPY --from=build /out/assistant /usr/local/bin/assistant
# fpcalc for the intro fingerprinting (tiny; pulls no ffmpeg of its own)
RUN apt-get update && \
    apt-get install -y --no-install-recommends libchromaprint-tools && \
    rm -rf /var/lib/apt/lists/*

# syntax=docker/dockerfile:1
# Release image: build the assistant, bake it into linuxserver/plex
# with the s6 services. Works for any platform via buildx.
#
# Other targets:
#   dist       the static binary alone (just build-linux)
#   fakecloud  the test rig's fake cloud drive (compose.dev.yaml)
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
    mkdir -p /out && cp target/release/assistant target/release/fakecloud /out/

FROM scratch AS dist
ARG TARGETARCH
COPY --from=build /out/assistant /assistant-linux-${TARGETARCH}

FROM scratch AS fakecloud
COPY --from=build /out/fakecloud /fakecloud
EXPOSE 8399
ENTRYPOINT ["/fakecloud"]

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
RUN apt-get update && \
    apt-get install -y --no-install-recommends ffmpeg && \
    rm -rf /var/lib/apt/lists/*

COPY packaging/s6-overlay/ /etc/s6-overlay/
COPY --from=build /out/assistant /usr/local/bin/assistant

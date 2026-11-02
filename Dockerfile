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
ENV ASSISTANT_VERSION=${VERSION}
# musl static binaries, so they run in any container like the Go build did
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
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
RUN apt-get update && \
    apt-get install -y --no-install-recommends ffmpeg && \
    rm -rf /var/lib/apt/lists/*

COPY packaging/s6-overlay/ /etc/s6-overlay/
COPY --from=build /out/assistant /usr/local/bin/assistant

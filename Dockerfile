# syntax=docker/dockerfile:1
# Release image: build the assistant, bake it into linuxserver/plex
# with the s6 services. Works for any platform via buildx.
FROM golang:1.26 AS build
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY . .
ARG VERSION=dev
RUN CGO_ENABLED=0 go build \
    -ldflags "-X github.com/PlexAssistant/PlexAssistant/internal/cli.Version=${VERSION}" \
    -o /out/assistant ./cmd/assistant

FROM lscr.io/linuxserver/plex:latest
RUN apt-get update && \
    apt-get install -y --no-install-recommends ffmpeg && \
    rm -rf /var/lib/apt/lists/*

COPY packaging/s6-overlay/ /etc/s6-overlay/
COPY --from=build /out/assistant /usr/local/bin/assistant

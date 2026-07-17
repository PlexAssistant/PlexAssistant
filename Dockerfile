# Dev image: linuxserver/plex plus ffmpeg for analysis probes.
# The release image (multi-stage build of the assistant binary) comes later.
FROM lscr.io/linuxserver/plex:latest

RUN apt-get update && \
    apt-get install -y --no-install-recommends ffmpeg && \
    rm -rf /var/lib/apt/lists/*

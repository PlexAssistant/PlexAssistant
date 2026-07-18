# Dev image: linuxserver/plex plus ffmpeg and the assistant s6 services.
# The release image (multi-stage build of the assistant binary) comes later.
FROM lscr.io/linuxserver/plex:latest

RUN apt-get update && \
    apt-get install -y --no-install-recommends ffmpeg && \
    rm -rf /var/lib/apt/lists/*

COPY packaging/s6-overlay/ /etc/s6-overlay/

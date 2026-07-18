#!/usr/bin/with-contenv bash
# Install scanner/transcoder symlinks and scan prefs once at boot.
BIN=/usr/local/bin/assistant
[ -x "$BIN" ] || BIN=/dist/assistant-linux-arm64
[ -x "$BIN" ] || exit 0 # nothing deployed yet
exec "$BIN" install

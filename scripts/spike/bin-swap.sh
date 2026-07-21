#!/usr/bin/env bash
# Swap an in-container Plex binary with the logging stub, or restore the
# original. The rename lives in the container layer: rerun "swap" after
# any container recreate.
# Usage: bin-swap.sh <binary-name> [swap|restore|log]
set -euo pipefail

BIN="${1:?binary name required, e.g. \"Plex Transcoder\"}"
CMD="${2:-swap}"
PLEX="/usr/lib/plexmediaserver/$BIN"

case "$CMD" in
swap)
  docker compose -f compose.dev.yaml exec -T plex bash -ec '
    PLEX="/usr/lib/plexmediaserver/'"$BIN"'"
    if [ ! -e "$PLEX.real" ]; then
      mv "$PLEX" "$PLEX.real"
    fi
    cp /spike/scanner-stub.sh "$PLEX"
    chmod +x "$PLEX"
    echo "stub active at: $PLEX"
  '
  ;;
restore)
  docker compose -f compose.dev.yaml exec -T plex bash -ec '
    PLEX="/usr/lib/plexmediaserver/'"$BIN"'"
    if [ -e "$PLEX.real" ]; then
      mv -f "$PLEX.real" "$PLEX"
      echo "original restored"
    else
      echo "nothing to restore"
    fi
  '
  ;;
log)
  docker compose -f compose.dev.yaml exec -T plex \
    cat "/config/spike/$(echo "$BIN" | tr ' ' '_').log" 2>/dev/null || echo "no log yet"
  ;;
*)
  echo "usage: $0 <binary-name> [swap|restore|log]" >&2
  exit 1
  ;;
esac

#!/usr/bin/env bash
# Swap the in-container "Plex Media Scanner" with the logging stub, or
# restore the original. The rename lives in the container layer: rerun
# "swap" after any container recreate.
# Usage: scanner-swap.sh [swap|restore|log]
set -euo pipefail

case "${1:-swap}" in
swap)
  docker compose -f docker-compose.dev.yml exec -T plex bash -ec '
    PLEX="/usr/lib/plexmediaserver/Plex Media Scanner"
    if [ ! -e "$PLEX.real" ]; then
      mv "$PLEX" "$PLEX.real"
    fi
    cp /spike/scanner-stub.sh "$PLEX"
    chmod +x "$PLEX"
    echo "stub active at: $PLEX"
  '
  ;;
restore)
  docker compose -f docker-compose.dev.yml exec -T plex bash -ec '
    PLEX="/usr/lib/plexmediaserver/Plex Media Scanner"
    if [ -e "$PLEX.real" ]; then
      mv -f "$PLEX.real" "$PLEX"
      echo "original restored"
    else
      echo "nothing to restore"
    fi
  '
  ;;
log)
  docker compose -f docker-compose.dev.yml exec -T plex cat /config/spike/scanner.log 2>/dev/null ||
    echo "no log yet"
  ;;
*)
  echo "usage: $0 [swap|restore|log]" >&2
  exit 1
  ;;
esac

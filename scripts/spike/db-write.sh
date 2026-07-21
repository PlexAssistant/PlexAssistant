#!/usr/bin/env bash
# Probe the running Plex library database: journal/schema state, online
# backup and lock behavior. Safe to run against a live Plex.
# Usage: db-write.sh [status|backup|lock-test]
set -euo pipefail

DB="/config/Library/Application Support/Plex Media Server/Plug-in Support/Databases/com.plexapp.plugins.library.db"
PSQL=(docker compose -f compose.dev.yaml exec -T plex "/usr/lib/plexmediaserver/Plex SQLite")

case "${1:-status}" in
status)
  echo "-- journal / schema version --"
  "${PSQL[@]}" "$DB" "PRAGMA journal_mode; SELECT max(version) FROM schema_migrations;"
  echo "-- WAL size --"
  docker compose -f compose.dev.yaml exec -T plex ls -la "$DB-wal"
  ;;
backup)
  echo "-- online backup + integrity --"
  "${PSQL[@]}" "$DB" ".backup /config/spike/library-backup.db"
  "${PSQL[@]}" /config/spike/library-backup.db "PRAGMA integrity_check;"
  ;;
lock-test)
  echo "-- hold a write txn for 3s, probe with busy_timeout=4000 --"
  docker compose -f compose.dev.yaml exec -T plex bash -c '
    DB="/config/Library/Application Support/Plex Media Server/Plug-in Support/Databases/com.plexapp.plugins.library.db"
    SQL="/usr/lib/plexmediaserver/Plex SQLite"
    ( echo "PRAGMA busy_timeout=5000; BEGIN IMMEDIATE; UPDATE library_sections SET name=name WHERE id=1;"; sleep 3; echo "COMMIT;" ) | "$SQL" "$DB" >/dev/null &
    sleep 0.5
    time "$SQL" "$DB" "PRAGMA busy_timeout=4000; BEGIN IMMEDIATE; UPDATE library_sections SET name=name WHERE id=1; COMMIT;"
    wait'
  ;;
*)
  echo "usage: $0 [status|backup|lock-test]" >&2
  exit 1
  ;;
esac

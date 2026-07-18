#!/bin/bash
# Observation stub: logs invocation details, then runs the real binary
# (.real sibling) and forwards its stdout and exit code.
# Log lands in /config/spike/<argv0-basename>.log (host: ./data/config/spike/).
LOG="/config/spike/$(basename "$0" | tr ' ' '_').log"
REAL="$0.real"
mkdir -p "$(dirname "$LOG")"
{
  echo "=== $(date -u +%FT%TZ) pid=$$ ppid=$PPID"
  echo "argv0=$0"
  i=0
  for a in "$@"; do
    i=$((i + 1))
    echo "argv[$i]=<$a>"
  done
  echo "cwd=$(pwd)"
  echo "parent=$(readlink /proc/$PPID/exe 2>/dev/null)"
  echo "parent_cmd=$(tr '\0' ' ' </proc/"$PPID"/cmdline 2>/dev/null)"
} >>"$LOG"
OUT=$("$REAL" "$@" 2>>"$LOG")
rc=$?
{
  echo "exit=$rc stdout_bytes=${#OUT}"
  echo "--- stdout ---"
  printf '%s\n' "$OUT"
  echo "--- /stdout ---"
} >>"$LOG"
printf '%s' "$OUT"
exit $rc

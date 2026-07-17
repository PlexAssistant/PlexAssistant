#!/usr/bin/env bash
# Generate a bulk strm sample set for stress testing.
# Usage: gen-bulk-strm.sh [count] [base-url] [out-dir]
set -euo pipefail

COUNT="${1:-500}"
BASE="${2:-http://fakecloud:8399}"
OUT="${3:-testdata}"

SHOW="Bulk Show (2026)"
SEASON="$OUT/strm/$SHOW/Season 01"
mkdir -p "$SEASON"

eps=(ep1.mkv ep2.mkv ep3.mkv)
for i in $(seq 1 "$COUNT"); do
  ep=$(printf 'S01E%03d' "$i")
  printf '%s/file/%s\n' "$BASE" "${eps[$(( (i - 1) % 3 ))]}" > "$SEASON/$SHOW - $ep.strm"
done
echo "generated $COUNT strm in $SEASON"

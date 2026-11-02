#!/usr/bin/env bash
# Generate a realistic bulk strm sample set for stress testing: two real
# shows with their real season/episode layouts, cycling the three test
# videos as targets.
# Usage: gen-bulk-strm.sh [base-url] [out-dir]
set -euo pipefail

BASE="${1:-http://fakecloud:8399}"
OUT="${2:-testdata}"

STRM="$OUT/strm"
eps=(ep1.mkv ep2.mkv ep3.mkv)
n=0

# show "folder name" season-episode-counts (real layouts)
gen_show() {
  local show="$1"
  shift
  local counts=("$@")
  for season in "${!counts[@]}"; do
    local dir
    dir=$(printf '%s/Season %02d' "$show" "$((season + 1))")
    mkdir -p "$STRM/$dir"
    for ep in $(seq 1 "${counts[$season]}"); do
      local code
      code=$(printf 'S%02dE%02d' "$((season + 1))" "$ep")
      printf '%s/file/%s\n' "$BASE" "${eps[$(( n % 3 ))]}" > "$STRM/$dir/$show - $code.strm"
      n=$((n + 1))
    done
  done
}

gen_show "Family Guy (1999)" 7 21 22 30 18 12 16 20 18 23 21 20 20 20 20
gen_show "The Walking Dead (2010)" 6 13 16 16 16 16 16 16 16 16 24

echo "generated $n strm under $STRM (two shows)"

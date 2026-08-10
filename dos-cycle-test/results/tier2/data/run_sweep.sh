#!/usr/bin/env bash
# Measures real guest cycles/decode via the decode-bench guest.
# For each size: run two K values, per-decode = (cyc(K2)-cyc(K1))/(K2-K1).
set -euo pipefail
cd "$(dirname "$0")/../.."
BIN=target/release/examples/decode_bench
APP=--app-bin-dir=artifacts/decode-bench/guest
OUT=artifacts/decode-bench/sweep.csv
echo "size_words,do_clone,k,cycles" > "$OUT"

run() { # size_words do_clone k
  local line
  line=$("$BIN" $APP --size-words "$1" --do-clone "$2" --k "$3")
  echo "$line"
  local cyc=${line##*cycles_executed=}
  echo "$1,$2,$3,$cyc" >> "$OUT"
}

# 2 MiB max-size (the optimal attack contract), no clone and with #93 clone
run 65535 0 64
run 65535 0 192
run 65535 1 64
run 65535 1 192
# scaling points
run 32768 0 64
run 32768 0 192
run 16384 0 64
run 16384 0 192
run 3072  0 128
run 3072  0 384

echo "=== wrote $OUT ==="
cat "$OUT"

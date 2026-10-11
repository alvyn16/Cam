#!/bin/sh
# End-to-end checks on this Mac: unit + UI smoke tests, synthetic exports,
# the scripted GUI run (record test pattern -> pause/resume -> stop -> editor
# -> export -> quit) and, with --screen, a real 3 s display recording + export.
# Outputs go to $GLIDE_TEST_DIR (default ~/Movies/glide-tests), never the repo.
set -eu
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/cargo-target/screenstudio-rs}"
G="$CARGO_TARGET_DIR/release/glide"
OUT="${GLIDE_TEST_DIR:-$HOME/Movies/glide-tests}/selftest"
rm -rf "$OUT"; mkdir -p "$OUT"
pass=0; fail=0
ok() { echo "PASS  $1"; pass=$((pass+1)); }
ko() { echo "FAIL  $1 (see $OUT)"; fail=$((fail+1)); }

cargo build --release -q
if cargo test --release -q >"$OUT/tests.log" 2>&1; then ok "cargo test ($(grep -o '[0-9]* passed' "$OUT/tests.log" | awk '{s+=$1} END {print s}') tests)"; else ko "cargo test"; fi

if "$G" synth "$OUT/synth.glide" --duration 6 >"$OUT/synth.log" 2>&1 \
   && "$G" export "$OUT/synth.glide" -o "$OUT/synth.mp4" >>"$OUT/synth.log" 2>&1 \
   && "$G" export "$OUT/synth.glide" --format gif -o "$OUT/synth.gif" >>"$OUT/synth.log" 2>&1 \
   && [ -s "$OUT/synth.mp4" ] && [ -s "$OUT/synth.gif" ]; then
  ok "synthetic package -> mp4 + gif"
else
  ko "synthetic export"
fi

if GLIDE_GUI_SELFTEST=1 GLIDE_OUTPUT_DIR="$OUT/gui" "$G" gui >"$OUT/gui.log" 2>&1 && grep -q 'selftest: PASS' "$OUT/gui.log"; then
  ok "gui app ($(grep -o 'elapsed=[0-9.]*s' "$OUT/gui.log"), $(grep -o 'frames: [0-9]*' "$OUT/gui.log"))"
else
  ko "gui app"
fi

if [ "${1:-}" = "--screen" ]; then
  if "$G" record -t 3 --dir "$OUT" --name screen >"$OUT/screen.log" 2>&1 \
     && "$G" export "$OUT/screen.glide" -o "$OUT/screen.mp4" >>"$OUT/screen.log" 2>&1 \
     && [ -s "$OUT/screen.mp4" ]; then
    ok "real display recording -> export"
  else
    ko "real display recording (Screen Recording permission?)"
  fi
fi
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]

#!/bin/sh
# End-to-end checks on this Mac: unit + UI smoke tests, synthetic exports,
# the background-settings pixel check, the scripted GUI run (record test
# pattern -> pause/resume -> stop -> editor -> export -> quit) and, with
# --screen, a real 3 s display recording + export + background check.
# Outputs go to $GLIDE_TEST_DIR (default ~/Movies/glide-tests), never the repo.
set -eu
cd "$(dirname "$0")/.."
. scripts/env.sh
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

# Every Background setting (wallpapers, blur, gradient, colour, image) and the
# padding, roundness and shadow around it must visibly change a rendered frame.
if "$G" check-background "$OUT/synth.glide" >"$OUT/background.log" 2>&1; then
  ok "background settings change the frame ($(grep -c '^ok' "$OUT/background.log") checks)"
else
  ko "background settings"
fi

# iPhone/iPad recordings (no device needed): portrait → phone frame → MP4.
if "$G" synth "$OUT/phone.glide" --device --duration 3 --width 1180 --height 2556 >"$OUT/phone.log" 2>&1 \
   && grep -q '"device": "phone"' "$OUT/phone.glide/project.json" \
   && "$G" export "$OUT/phone.glide" -o "$OUT/phone.mp4" >>"$OUT/phone.log" 2>&1 && [ -s "$OUT/phone.mp4" ]; then
  ok "synthetic iPhone recording -> phone frame + mp4"
else
  ko "synthetic iPhone recording"
fi

code=0
GLIDE_GUI_SELFTEST=1 GLIDE_OUTPUT_DIR="$OUT/gui" "$G" gui >"$OUT/gui.log" 2>&1 || code=$?
echo "exit status $code" >>"$OUT/gui.log"
if [ "$code" -eq 0 ] && grep -q 'selftest: PASS' "$OUT/gui.log"; then
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
  if [ -d "$OUT/screen.glide" ] && "$G" check-background "$OUT/screen.glide" >"$OUT/screen-background.log" 2>&1; then
    ok "background settings on the real recording ($(grep -c '^ok' "$OUT/screen-background.log") checks)"
  else
    ko "background settings on the real recording"
  fi
  # Resize Glide's own window (only that exact window, by its process) through the
  # Accessibility API, as the Window source's Resize menu does.
  if "$G" permissions 2>/dev/null | grep -q 'Accessibility: *Authorized'; then
    GLIDE_OUTPUT_DIR="$OUT/gui" "$G" gui >"$OUT/resize.log" 2>&1 &
    gpid=$!
    # Find its window by process id (CoreGraphics; no ScreenCaptureKit).
    resized=""
    for _ in 1 2 3 4 5 6 7 8 9 10; do
      sleep 1
      if "$G" resize-window "pid:$gpid" 1024x700 >>"$OUT/resize.log" 2>&1; then resized=1; break; fi
    done
    if [ -n "$resized" ] && grep -q '→ 1024×700' "$OUT/resize.log"; then
      ok "window resize (Accessibility)"
    else
      ko "window resize"
    fi
    kill "$gpid" 2>/dev/null; wait "$gpid" 2>/dev/null || true
  else
    echo "SKIP  window resize (no Accessibility permission)"
  fi
fi
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]

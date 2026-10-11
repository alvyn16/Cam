# Progress

Product name **Glide** (crate and binary `glide`, bundle `com.heyfabrika.glide`).
Milestones track the parity checklist in `analysis/SCREEN_STUDIO.md`.

| # | Milestone | Status |
|---|---|---|
| M0 | Reverse engineering with REA, parity checklist | ☑ `analysis/SCREEN_STUDIO.md` |
| M1 | Engine: springs, deterministic animator, timeline map, sizes, auto zoom + targeting, cursor model, click effects, camera layouts, shortcut badges, captions | ☑ unit tested |
| M2 | GPU renderer (wgpu/Metal): background, shadow, rounded window + inset, cursor, click FX, motion blur, camera, text | ☑ |
| M3 | Media: AVAssetReader decode with seeking and prefetch | ☑ |
| M4 | Recording: ScreenCaptureKit display/window/area, system audio, mic, camera, input + cursor logging | ☑ (capture core from rec-rs) |
| M5 | Export: MP4 (VideoToolbox H.264/HEVC, zero-copy), audio mix with cuts/speed, GIF | ☑ |
| M6 | Editor UI (egui + Inter): library, preview, inspector, timeline, recording widget, menu bar, hotkey | ☑ |
| M7 | Captions (Apple Speech, on device), shortcut overlay | ☑ |
| M8 | App bundle, icon, signing (`scripts/build.sh`) | ☑ |
| M9 | Verification with real recordings; `PERFORMANCE.md` | ☑ `scripts/selftest.sh --screen`, `scripts/bench.sh` |

## Verification on this MacBook (M4 Pro, macOS 27)

`scripts/selftest.sh --screen` (2026-10-11): 4/4 pass.

- `cargo test`: engine, project, renderer, transcription chunking, icon and a
  headless UI smoke test that drives every editor panel.
- Synthetic package → MP4 and GIF.
- Scripted GUI run (`GLIDE_GUI_SELFTEST=1`): launch → record (test pattern) →
  pause/resume → stop → editor opens the new project → export → quit.
- Real 5K display recording (Screen Recording permission granted to the
  terminal) → export.

UI layouts are checked with `glide snapshot` (renders the real UI headlessly
to PNG) for the home view, every inspector tab, zoom selection and the
export sheet, in light and dark mode.

## What Micky needs to do (permissions)

The app bundle has its own privacy identity (`com.heyfabrika.glide`), signed
with the Apple Development identity so grants survive rebuilds.

1. `scripts/build.sh`, then `open dist/Glide.app`.
2. Press **Request access** on the home screen (or start a recording). Then in
   **System Settings › Privacy & Security**:
   - **Screen & System Audio Recording**: enable **Glide** (needed for any
     screen, window or area recording and for system audio). Quit and reopen
     Glide afterwards; macOS only applies this on relaunch.
   - **Microphone**: enable **Glide** (prompted the first time a recording
     uses the microphone).
   - **Camera**: enable **Glide** (prompted the first time a recording uses
     the camera).
   - **Input Monitoring**: add/enable **Glide** to record keyboard shortcuts
     for the shortcut overlay (mouse and cursor logging need no permission).
   - **Speech Recognition**: allow when prompted by *Transcribe microphone*
     in the Captions tab.
3. Optional end-to-end check from Terminal (uses the terminal's own grants):
   `scripts/selftest.sh --screen` and `scripts/bench.sh`.

Already verified without your input: Screen Recording works from the terminal
(real 5K recordings and exports above). Microphone and Camera have not been
granted on this Mac yet, so mic/camera capture were exercised only through
the code paths shared with rec-rs and synthetic audio in export tests.

## Log

- 2026-10-11: REA analysis of Screen Studio 3.7.1; engine core with tests.
- 2026-10-11: wgpu renderer, AVFoundation decode, VideoToolbox/GIF export,
  recorder with input logging; zero-copy export path.
- 2026-10-11: egui app (library, editor, recording widget, menu bar, hotkey),
  on-device captions, Glide.app bundle with generated icon, self-tests,
  performance measurements.

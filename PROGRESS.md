# Progress

Product name **Glide** (crate and binary `glide`, bundle `com.heyfabrika.glide`).
Milestones and parity track the checklist in `analysis/SCREEN_STUDIO.md`.

| # | Milestone | Status |
|---|---|---|
| M0 | Reverse engineering with REA, parity checklist | ☑ `analysis/SCREEN_STUDIO.md` |
| M1 | Engine: springs, deterministic animator, timeline map, sizes, auto zoom + targeting, cursor model, click effects, camera layouts, shortcut badges, captions | ☑ unit tested |
| M2 | GPU renderer (wgpu/Metal): background, shadow, rounded window + inset, cursor, click FX, motion blur, camera, text, masks, device frames | ☑ |
| M3 | Media: AVAssetReader decode with seeking and prefetch | ☑ |
| M4 | Recording: ScreenCaptureKit display/window/area, system audio, mic, camera, input + cursor logging, iOS devices | ☑ (capture core from rec-rs) |
| M5 | Export: MP4 (VideoToolbox H.264/HEVC, zero-copy), audio mix with cuts/speed/voice-over ducking, GIF | ☑ |
| M6 | Editor UI (egui + Inter): library, preview, inspector, timeline, recording widget, area picker, menu bar, shortcuts | ☑ |
| M7 | Captions (Apple Speech, on device, editable), shortcut overlay | ☑ |
| M8 | App bundle, icon, signing (`scripts/build.sh`) | ☑ |
| M9 | Verification with real recordings; `PERFORMANCE.md` | ☑ `scripts/selftest.sh --screen`, `scripts/bench.sh` |

## Feature parity

☑ done and verified on this Mac · ◐ done with a stated gap · ☐ not done ·
✗ deliberately different (see "Deliberate differences" in the analysis).

| ID | Feature | Status | Notes |
|---|---|---|---|
| R1 | Display recording | ☑ | Native Retina size, display refresh, cursor hidden (re-rendered later). |
| R2 | Window recording | ☑ | Window bounds logged over time; cursor stays aligned when the window moves. |
| R3 | Area recording | ☑ | Drag-to-select picker (move/resize, Return/Esc), size presets, exact values. |
| R4 | System audio | ☑ | Own track (stems). |
| R5 | Microphone | ◐ | Own track; "Enhance voice" (high-pass, gate, levelling) in the editor. No capture-time AGC. |
| R6 | Camera | ◐ | Separate 720p file, bubble/layouts in the editor. Not exercised live: Camera permission not granted on this Mac. |
| R7 | iOS device recording | ◐ | CoreMediaIO screen-capture devices, native-size HEVC, phone/tablet frame. Untested: no iPhone/iPad was connected. |
| R8 | Input logging | ☑ | 120 Hz mouse + buttons, cursor images with hotspots, keystrokes (needs Input Monitoring). |
| R9 | Countdown, pause/resume, restart, cancel | ☑ | Floating widget; verified by the scripted GUI run. |
| R10 | Hide desktop icons / Dock | ☑ | Excluded from the capture by window layer; wallpaper kept. Verified on a real recording. |
| R11 | Global shortcuts | ☑ | Start/stop, pause, cancel; re-bindable; conflicts reported. |
| R12 | Permissions | ☑ | Status, request button, exact steps below. |
| Z1–Z7 | Auto zoom, zoom model, targeting, manual target, edge snapping, spring zoom, motion blur | ☑ | Engine unit tests; manual target picked by clicking the preview. |
| C1–C9 | Cursor re-rendering, smoothing, tilt, press, idle hide, hidden slices, click circle/ripple, click sounds, loop/freeze | ☑ | System cursor images, plus generated vector and touch skins. |
| S1 | Backgrounds | ☑ | Generated wallpapers, gradients, colour, imported image, current desktop picture, blur. |
| S2–S7 | Padding, roundness, shadow, inset, aspect ratio, crop | ☑ | |
| S8 | Device frames | ☑ | Generated laptop, phone, tablet frames (no artwork reused). |
| K1–K5 | Camera bubble, shape, shrink while zoomed, layouts, auto-hide | ◐ | Implemented and rendered from test footage; live camera capture unverified (permission). |
| T1–T7 | Slices, trim, split/delete, speed, zoom track, layout track, undo/redo/scrub/loop | ☑ | |
| T8 | Typing speed-up | ☑ | Detects typing runs in the key log; one click to 2–4×. |
| T9 | Masks | ☑ | Blur, pixelate, highlight; drawn on the preview; follow zoom. |
| T10 | Voice-over | ◐ | Recorded with AVAudioRecorder, mixed with ducking (mixing verified with a test file). Live capture needs Microphone permission. |
| O1 | Shortcut overlay | ☑ | |
| O2 | Captions | ◐ | Apple Speech on device, word timing, editable lines. Needs Speech Recognition permission (run from Glide.app). No whisper.cpp option. |
| A1–A2 | Mix, background music | ☑ | Per-slice volumes, mutes, master volume, looped music with fade. |
| A3 | Enhance microphone | ◐ | DSP clean-up, not an ML denoiser. |
| E1–E3 | MP4, resolution, frame rate | ☑ | Hardware H.264/HEVC, quality presets by bits per pixel. |
| E4 | GIF | ☑ | Parallel NeuQuant palettes, delta frames (only changed tiles, transparent elsewhere), duplicate merging, loop option. |
| E5 | Targets | ✗ | File, clipboard, reveal in Finder. No upload/share links (no accounts). |
| E6 | Presets / size estimate | ☑ | |
| E7 | Progress, cancel, quick export | ☑ | "Export & copy" banner after recording. |
| E8 | Size-targeted export | ☑ | Not in Screen Studio 3.7.1; Glide's "Fit to file size" (MP4) stays at or under the target, re-encoding once if the first pass overshoots. |
| P1 | Projects library | ☑ | `~/Movies/Glide`, autosave, rendered thumbnails. |
| P2 | Style presets | ☑ | Save/apply/delete, default preset for new recordings. |
| P3 | Editor sidebar + selection inspector | ☑ | |
| P4 | Menu bar | ☑ | Record display/window/area, pause, status with elapsed time. |

## Verification on this MacBook (M4 Pro, macOS 27)

`scripts/selftest.sh --screen` (2026-10-11): 4/4 pass.

- `cargo test` (66): engine, project, presets, renderer helpers, masks,
  device frames, typing detection, caption editing, transcription chunking,
  hotkeys, icon, plus a headless UI smoke test driving every editor panel.
- Synthetic package → MP4 and GIF.
- Scripted GUI run (`GLIDE_GUI_SELFTEST=1`): launch → area picker opens over
  the display (screenshotted) and is confirmed → record (test pattern) →
  pause/resume → stop → editor opens the new project → export → quit.
- Real 5K display recording (Screen Recording granted to the terminal) →
  export. Masks, device frames and voice-over mixing checked on real
  footage with `glide frame` and ffmpeg.

UI layouts are checked with `glide snapshot`, which renders the real UI
headlessly to PNG (home, every inspector tab, zoom/mask selection, export
sheet, post-recording banner; light and dark).

## What Micky needs to do (permissions)

The app bundle has its own privacy identity (`com.heyfabrika.glide`), signed
with the Apple Development identity so grants survive rebuilds.

1. `scripts/build.sh`, then `open dist/Glide.app`.
2. Press **Request access** on the home screen (or start a recording). Then in
   **System Settings › Privacy & Security**:
   - **Screen & System Audio Recording**: enable **Glide** (needed for any
     screen, window or area recording and for system audio). Quit and reopen
     Glide afterwards; macOS only applies this on relaunch.
   - **Microphone**: enable **Glide** (prompted the first time a recording or
     voice-over uses the microphone).
   - **Camera**: enable **Glide** (prompted the first time a recording uses
     the camera).
   - **Input Monitoring**: add/enable **Glide** to record keyboard shortcuts
     for the shortcut overlay and typing speed-up (mouse and cursor logging
     need no permission).
   - **Speech Recognition**: allow when prompted by *Transcribe microphone*
     in the Captions tab.
3. To try iPhone/iPad recording: connect the device with a cable, unlock it,
   tap *Trust*, then pick **iPhone** as the source (`glide devices` lists it).
4. Optional end-to-end check from Terminal (uses the terminal's own grants):
   `scripts/selftest.sh --screen` and `scripts/bench.sh`.

Already verified without your input: Screen Recording works from the terminal
(real 5K recordings and exports above). Microphone, Camera and Speech
Recognition have not been granted on this Mac, so live mic/camera/voice-over
capture and transcription were exercised only through their code paths and
test media.

## Log

- 2026-10-11: REA analysis of Screen Studio 3.7.1; engine core with tests.
- 2026-10-11: wgpu renderer, AVFoundation decode, VideoToolbox/GIF export,
  recorder with input logging; zero-copy export path.
- 2026-10-11: egui app (library, editor, recording widget, menu bar, hotkey),
  on-device captions, Glide.app bundle with generated icon, self-tests,
  performance measurements.
- 2026-10-11: style presets, desktop-wallpaper background, typing speed-up,
  loop playback, caption editing, masks, area picker, hide icons/Dock,
  configurable shortcuts, device frames, voice-over, iOS device capture,
  menu bar sources, quick export, Finder document opening, GIF delta frames.

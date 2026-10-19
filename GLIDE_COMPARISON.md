# Cam compared with Glide

Current review: 2026-10-18. Upstream HEAD was checked again and remains `2d4ce59179d7784dae840961424d056cf7919486`: [Glide source](https://github.com/michaelshimeles/glide/tree/2d4ce59179d7784dae840961424d056cf7919486). The original failures are preserved below as a historical audit; they do not describe the current implementation. Full evidence and runtime requirements are in [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md).

| Capability | Current Windows implementation | Verification |
|---|---|---|
| Display/window/area recording | Real WGC capture, moving-window geometry, pause/restart/cancel, physical DPI coordinates, occluded classic-window fallback | Real recordings independently decoded; 100%/125% picker checks and moving/occluded window checks |
| System audio, microphone, camera, narration | Real WASAPI/DirectShow streams, separate stems, preview release before recording, process-excluding loopback | Connected Realtek audio and Integrated Camera; tone round trips, voice-over, actual GUI recording |
| Floating camera monitor (added beyond Glide) | Optional Show while recording switch below Camera; shared recording stream, draggable monitor, independent hide, live during pause, excluded from screen capture | Dark/light rendered UI, real recording/pause frame delivery, native window checks and optimized GUI acceptance |
| Native controls and input | Hotkeys/conflicts, floating bar, tray, cursor images, keyboard logging; Windows shortcut labels and modifier filtering | Native controls and focused injected-key/cursor check; input/typing regressions |
| Editor styling and effects | Zoom/cursor animation, backgrounds, frames, masks, timeline edits, camera layouts, undo/redo, music/ducking and captions retained | Engine/render/UI regressions, actual camera bubble, decoded edited exports |
| Rapid timeline scrubbing | Background decoding, latest-seek replacement, bounded frame queues and audio clock-only seeking | Real-project renderer and native window-message checks; zero UI timeouts, worst measured response 10.8 ms |
| High-resolution capture/editing | Display/area WGC textures feed the native hardware encoder without full-frame CPU readback. High-resolution previews use persistent asynchronous Media Foundation decoding, explicit GPU color conversion and antialiased downsampling | 4K texture encoding and decoded-pixel/seek/cancellation checks; exact final performance and release evidence in IMPLEMENTATION_STATUS.md |
| Video export | Complete standard H.264/HEVC/ProRes/GIF output; native compositor-to-hardware encoder bridge, quality and size targets, safe cancellation/fallback | Both native 4K codecs, independent full decoding, quality-detail and file-size tests; repeated-job/resource checks |
| Long audio projects | Disk-backed capture/editor/export audio with fixed sample windows | Real 120-second capture; hour-long editor/playback/export acceptance |
| Transcription and diagnostics | Bounded decoding/chunks, reused Whisper state, original word times, cancellation and atomic transcript save | Known 14-word speech; 42 correct words at three points across one hour; bounded-memory `audio-check` |
| Read-only projects/cache use | Writable application cache; 2 GiB eviction target with active-file leases, safe concurrent publication and repair | ACL-denied-write project preview/export with all source hashes unchanged; cache lifetime/concurrency regressions |

The broad verification on 2026-10-18 passed 142 standard tests and 18 explicit hardware tests. Subsequent microphone, setup and high-resolution changes have their own exact build identities and acceptance evidence in IMPLEMENTATION_STATUS.md. The application and Windows setup are published on GitHub Releases. Historical test counts below describe the earlier snapshots.

## Remaining platform differences

- Direct USB iPhone/iPad screen capture is not implemented through Apple's macOS CoreMediaIO transport. A Windows DirectShow driver-exposed stream can be selected; pairing/trusting an iPhone alone does not create one. No iPhone/iPad is available here, so this is not claimed as verified parity.
- Native export and accelerated display/area capture avoid full-frame CPU readback. Interactive native preview decoding/color conversion/downsampling run on the GPU, but reduced preview pixels still pass through CPU memory before compositor upload. Classic-window capture and ProRes/software recording retain their established CPU paths. Fully shared decoder-to-compositor textures remain a separate optimization.
- Windows uses Whisper for captions and RNNoise for voice isolation instead of Apple Speech/AUSoundIsolation. Base Whisper and the voice-processing path were verified; small/medium model weights were not downloaded for this review.
- Other GPU vendors, Windows versions and unavailable devices are capability-probed but not claimed as live-verified. Capture device hotplug recovery is not automatic; fatal stream errors stop capture with an error. Playback output recovery remains supported.

## Original pre-fix findings (2026-10-17)

**Historical pre-fix audit.** For the implemented fixes, current test evidence, runtime requirements and remaining platform differences, see [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md).

Reviewed 2026-10-17. Cam commit: `8387e29381af9ddef3df899227013763509042a9`, including the existing uncommitted working-tree changes. Upstream Glide main: `2d4ce59179d7784dae840961424d056cf7919486`.

Baseline: https://github.com/michaelshimeles/glide/tree/2d4ce59179d7784dae840961424d056cf7919486

## Assessment

Cam preserves much of Glide's editor, animation engine, and rendering work, but the Windows capture and media implementation is incomplete. It currently behaves as an editor/demo foundation rather than a functioning screen recorder. Several placeholders return success, which hides missing functionality from the UI and tests.

This review combines source comparison, all-target Rust tests, and CLI probes on Windows. It does not claim interactive GUI or live camera/transcription validation. Glide's own progress document marks live camera, iOS capture, and voice-over as partially verified; upstream documentation is not proof that every upstream feature is flawless.

## Confirmed failures

### 1. Critical: recording succeeds without recording video

`src/capture/record.rs:60`: `Session::start` only stores the output path and start time. It never starts capture or a writer. `has_frames()` always returns true, `t0_secs()` returns zero, and `finish()` returns empty statistics. The separate backend in `src/platform/windows/capture.rs:107` also ignores the source and frame callback.

Reproduced: `cam record --duration 0.3 --no-system-audio --dir <scratch> --name Capture` exits successfully and reports a recording. Its recording folder contains only `meta.json`, `mouse.json`, `keys.json`, and `cursors.json`; `screen.mov` is absent. Opening/exporting that recording cannot recover video that was never saved.

### 2. Critical: MP4/MOV export is not encoded video

`src/media/writer.rs:76`: `push_frame` retains only the first frame. `finish` writes the custom magic bytes `CAMF`, dimensions, raw pixels, and optional raw audio. Codec, bitrate, and quality are not used by this writer. H.264, HEVC, and ProRes selections therefore do not produce those formats; file-size targeting cannot provide its advertised encoder behavior either.

Reproduced: a 0.5-second synthetic package exported to `demo.mp4` reports five exported frames and success. External `ffprobe` rejects it with `moov atom not found` and `Invalid data found when processing input`. The same package exported as GIF is recognized as GIF with duration 0.500 seconds.

`MovieWriter::create` and `finish` additionally discard filesystem errors, so an unwritable destination can be reported as success.

### 3. High: real video decoding is missing; screen footage is replaced by a demo

`src/media/mod.rs:67`: the reader recognizes the custom CAMF format, not an MP4/MOV decoder. At line 90 it classifies every file named `screen.mov` as synthetic, then generates `synth::screen_bgra` frames instead of decoding its contents. Actual recorded media would therefore show demo footage even after capture is implemented. Other real video filenames yield no decoded frames, and duration is hardcoded to ten seconds.

The separate `src/platform/windows/media.rs` reader/writer is also placeholder code and does not repair the path used by the app.

### 4. High: area selection and multi-monitor geometry are wrong

`src/platform/windows/capture.rs:44-45` stores monitor origin `(left, top)` in `DisplayInfo.points`. Callers expect logical width and height: `src/recorder/mod.rs:211` clamps the area against those values and line 213 divides pixel width by the first value.

Reproduced: `cam record --area 100,100,320,200 --duration 0.3 ...` fails with `the selected area is outside this display` on the primary monitor. An origin of `(0,0)` becomes a zero-sized display.

There are related independent gaps: `src/recorder/input.rs:52` ignores display ID and always returns primary-screen bounds; `src/app/os.rs` uses monitor handles while capture enumeration uses list indices; `src/capture/screen.rs:99` supplies fake zero window positions/PIDs and assigns every window to the first monitor. `winsize::front_to_back()` returns an empty list and display sizes are hardcoded to 1920x1080. The on-screen picker cannot reliably highlight actual window positions or choose the frontmost window.

### 5. High: live preview never supplies frames

`src/capture/preview.rs:28` ignores the source and callback. Both `take_frame` and `take_buffer` always return None. System level is never populated.

Reproduced: `cam preview --secs 0.2 -o <scratch>/preview.png` fails with `no preview frames`.

### 6. High: system/microphone recording and voice-over are missing

`src/platform/windows/audio.rs:45,56`: both capture methods return inactive implementations without creating streams or invoking sample callbacks. The main recording session does not perform audio capture either. `src/app/voiceover.rs:14` starts only a timer and never opens a microphone or writes an audio file.

Microphone enumeration and the independent level meter in `src/capture/meter.rs` do contain real cpal code. A moving meter would not establish that microphone audio is being saved.

### 7. High: webcam and iPhone/iPad capture are missing

`src/capture/camera.rs:12,25`: camera and iOS device lists are always empty. Camera start methods do nothing, and device dimensions always return None. Camera styling/layout code exists but has no captured video source.

### 8. High: global shortcuts and recording controls are placeholders

`src/app/hotkey.rs:64` stores callbacks but never registers Windows hotkeys or dispatches them. It retains macOS key codes and modifier constants. `src/app/home.rs:75` also retains the Win+Shift+Alt+R equivalent default, contradicting README's Ctrl+Shift+R claim.

`src/app/recbar.rs` creates no native window; update does nothing and its window ID is zero. `src/app/menubar.rs` creates no tray icon/menu. Countdown state exists in orchestration, but the floating bar that should display it is absent. The capture session's pause/resume methods are no-ops and elapsed time includes pauses.

### 9. High: typing speed-up and shortcut overlays have no live keyboard data

`src/recorder/input.rs:74`: the keys vector is created and later returned, but nothing populates it. Mouse position/button polling exists; key logging and cursor bitmap capture do not. All mouse events use cursor ID zero, and the cursor image list stays empty. The rendering/typing engines exist, but actual recordings cannot provide the required keyboard or cursor-shape data.

### 10. Medium: audio imports are much narrower than the file picker promises

`src/app/inspector.rs:489` accepts MP3, M4A, WAV, AAC, and AIFF. `src/media/mod.rs:228` only reads CAMF or limited WAV data, returning an empty buffer for other existing files. Thus most music imports silently contribute no sound. Track index is ignored; `audio_track_count` returns one for any existing file. WAV sample rates are not resampled to the 48 kHz expected by the mixer.

### 11. Medium: several Windows integrations misrepresent their behavior

- `src/app/os.rs:45`: Export & Copy copies a pathname as text, not a file payload usable as a pasted video attachment.
- `src/capture/tcc.rs`: microphone/camera status always says granted, irrespective of Windows privacy settings; even Display for Denied formats as granted.
- Hide icons/taskbar options are passed to the empty capture session and never applied.
- `src/capture/winsize.rs:19`: resize discards SetWindowPos errors and returns the requested size without reading back the actual result; PID lookup always returns None.
- `src/render/interop.rs:16`: texture cache construction always returns None, so Glide's zero-copy media/GPU path is absent.
- `src/export/isolate.rs`: ML voice isolation is absent. Basic DSP enhancement and gain control remain in `export/audio.rs`, so Enhance voice is partial rather than entirely missing.

### 12. High: diagnostic recording and graceful CLI shutdown are incomplete

`src/recorder/mod.rs:193` gives Pattern no session, while `stop` at line 449 expects a session and panics. Pattern recordings report no frames and zero elapsed time. `src/capture/pattern.rs` is also a stub.

`src/main.rs:613` defines STOP and reads it during recording, but no Ctrl-C/console handler sets it. The advertised Ctrl-C path does not gracefully finalize the package through this loop.

## Features substantially retained

The entire `src/engine` directory matches the reviewed Glide revision when ignoring line endings. This retains spring animation, click-cluster zoom targeting, manual/follow zoom models, cursor animation, timeline time mapping, camera layout models, captions, masks, and typing detection algorithms.

Editor controls for split/trim, speed, zoom/mask/layout tracks, undo/redo, style presets, backgrounds, padding, corners, shadows, device frames, caption editing, project persistence, and recent recordings remain substantially present. Rendering and editor smoke tests pass with synthetic footage. GIF export was independently recognized by ffprobe. These are useful foundations, but engine/UI presence does not mean the capture-to-export workflow works.

Whisper transcription remains a real implementation with model download and inference code. It was not exercised with speech in this review and is blocked for newly captured microphone audio by the missing capture path. Apple Speech was removed; that is an expected platform difference rather than a Windows defect.

## Verification and test gaps

`cargo test --all-targets --no-fail-fast`: **74 passed, 0 failed** (65 library, 3 audio, 3 background, 2 resampling, 1 UI smoke).

Passing tests currently overstate media readiness: export tests feed output into Cam's own CAMF-aware reader, so invalid MP4 files can pass audio and pixel checks. Synthetic `screen.mov` regeneration also avoids exercising actual decoding. The UI smoke test uses generated input/textures and does not establish that a native capture preview, floating control bar, tray icon, or global hotkey works.

The scripts directory still contains macOS workflows: selftest uses a `release/glide` binary and `.glide` paths; build/environment scripts assume Apple tooling; test-window.js imports Cocoa/AVFoundation. These are not usable Windows end-to-end verification.

Scratch outputs: `%LOCALAPPDATA%/Temp/cam-audit-20261017/`. Test log: `%TEMP%/cam-review-tests-20261017.log`. No application source was changed by this review.

## Recommended order

1. Implement one real display capture path and save an externally decodable recording. Fail explicitly for unavailable backends.
2. Implement real decoding and encoding; remove filename-based synthetic substitution. Verify changing frames, audio, duration, and codec with an independent decoder.
3. Fix monitor IDs, dimensions, DPI coordinates, and window position/process/z-order metadata.
4. Add system/microphone capture, pause synchronization, and real source preview; then webcam and voice-over.
5. Implement native hotkeys, floating recording controls, tray integration, input/cursor capture, and clipboard file transfer.
6. Add Windows end-to-end checks and revise README claims to match verified behavior. Validate capture → editor → export using real moving content and an audible test tone.

<img src="docs/screenshots/icon.png" width="96" alt="Cam icon">

# Cam

**A fast, native Windows screen recorder and video studio, built in Rust.**

Cam is a high-performance, native Rust screen recording and video editing application tailored specifically for Windows. Record your entire display, a specific window, or a cropped area with smooth frame rates. Cam automatically follows mouse movement, generates spring-animated zooms on clicks, re-renders high-DPI cursors, frames recordings with customizable backgrounds, padding, and rounded corners, and exports polished MP4 videos and GIFs.

![Editor](docs/screenshots/editor.png)

## Features

- **Native Windows Capture**:
  - Windows Graphics Capture (WGC) for native display, window, and region capture.
  - Record full displays, individual windows (tracked front-to-back), or custom regions.
  - Multi-track audio: WASAPI system audio loopback and microphone stems recorded separately.
  - Webcam capture with live, draggable camera bubble overlay.
  - Optional **Show while recording** switch below Camera keeps a draggable camera preview visible above other apps. Close the preview to hide it while the camera keeps recording; the preview window is excluded from screen capture.
  - Clean options to hide desktop icons and the Windows taskbar during capture.
  - Configurable global hotkeys (default `Ctrl+Shift+R`) to start, pause, and stop recording.
- **Automatic Zoom & Spring Physics**:
  - Smooth spring-animated zooms focused on mouse click clusters.
  - Edge snapping and motion blur for seamless focus shifts.
  - Configurable manual and follow-mouse zooms.
- **High-DPI Cursor Re-rendering**:
  - Silky cursor motion smoothing, tilt based on travel speed, press and idle animations.
  - Click ripples and optional sound effects.
- **Studio Styling & Device Frames**:
  - Use your current Windows desktop wallpaper, custom gradients, solid colors, custom images, and blur effects.
  - Customizable padding, corner roundness, shadows, aspect ratios, and device frames (Laptop, Tablet, Phone).
  - Instant live GPU preview in the editor with real-time parameter feedback.
- **Unified One-Window Workflow**:
  - Starts with an integrated setup view: live source preview, quick-pick window/display selector, audio meters, and a prominent 88px Record button.
  - When recording finishes, seamlessly transitions straight into the video timeline editor.
  - Recent recordings popover directly accessible from the title bar.
- **Editor & Captions**:
  - Non-destructive timeline with video clips, zoom keys, mask tracks, and camera layout tracks.
  - Blur, pixelate, and highlight masks with smart motion detection warnings ("data may be visible").
  - On-device speech-to-text captions using Whisper (`base`, `small`, `medium`).
  - Voice-over recording with automatic background audio ducking.
- **Video Export**:
  - Standard MP4 video export (H.264 / HEVC), ProRes MOV, and delta-frame GIFs. Available NVIDIA, Intel, or AMD encoders are probed at runtime; software encoding is used when hardware is unavailable or the frame is too small. Set `CAM_ENCODER=software` to force the software path.
  - Presets from 720p to 4K, 30 to 60 fps, with file-size targeting.
  - "Export & Copy" to place finished videos directly on the Windows clipboard.

![Recording setup with live preview](docs/screenshots/home.png)
![Editor light mode](docs/screenshots/editor-light.png)

## Getting Started

### Prerequisites

- **OS**: Windows 11 (verified). Older Windows capture API combinations have not been validated.
- **Toolchain**: Rust 2024 edition (`rustup update stable`)
- **DirectX**: DirectX 11 / 12 capable GPU
- **Media runtime**: FFmpeg and ffprobe on PATH, or beside cam.exe. Override locations with `CAM_FFMPEG` and `CAM_FFPROBE`. The FFmpeg build needs libx264/libx265, ProRes, AAC, DirectShow, and the desired hardware encoder.
- **Devices**: Enable desktop-app microphone and camera access in Windows Settings. iPhone/iPad video requires a Windows camera/capture driver that exposes the device through DirectShow; USB pairing alone does not provide screen capture.

### Running Cam

To launch the graphical app:

```pwsh
cargo run --release
```

Or run the debug build:

```pwsh
cargo run
```

### Building the Binary

Compile the standalone binary `cam.exe`:

```pwsh
cargo build --release
```

The resulting executable will be in `target/release/cam.exe`.

## CLI Usage

Cam includes a comprehensive command-line interface for headless recording, synthetic testing, and batch processing:

```pwsh
# Launch the GUI (default)
cam

# List available displays, windows, microphones, and cameras
cam devices

# Record the main display for 10 seconds into a new project
cam record --duration 10

# Record a specific window
cam record --window 12345 --duration 5

# Record an area (x,y,w,h in points)
cam record --area 100,100,1280,720

# Export a project to MP4
cam export path/to/Recording.cam --format mp4 --quality studio

# Transcribe microphone audio in a package using Whisper
cam transcribe path/to/Recording.cam --engine whisper-base

# Generate a synthetic demo package for layout testing
cam synth demo.cam --duration 10
```

## Storage & Package Format

- **Default Directory**: New recordings are saved in your user Videos directory:
  ```text
  %USERPROFILE%\Videos\Cam\
  ```
- **Package Format (`.cam`)**: Projects are self-contained directory bundles:
  ```text
  Recording 2026-10-17 at 19.30.00.cam/
    project.json        # Editor style, timeline cuts, zooms, masks, captions
    recording/
      meta.json         # Capture metadata, source resolution, and duration
      screen.mov        # Primary screen video track
      camera.mov        # Camera video track (if enabled)
      system.wav        # System audio loopback track
      mic.wav           # Separate microphone track
      mouse.json        # High-rate mouse position & click events
      keys.json         # Keystrokes
      transcript.json   # Generated Whisper captions
  ```

## Architecture

- **GUI**: Built with `egui` rendered through `wgpu` (DirectX 12 / Vulkan backend) on top of `winit`.
- **Capture**: Windows Graphics Capture through windows-capture; native Windows monitor/window geometry and input logging.
- **Media**: FFmpeg decodes standard media, resamples audio to 48 kHz stereo, and encodes standard MP4/MOV files. Native H.264/HEVC export shares compositor textures with a matching-adapter Media Foundation hardware encoder without compositor readback. Each export has its own worker process and Windows cleanup job. Source decoding/upload and capture still use CPU transfers. Unsupported native configurations retain the FFmpeg route. Set `CAM_NATIVE_ENCODER=require` to require native export or `off` to select FFmpeg; `CAM_ENCODER=software` forces software encoding.
- **Voice enhancement**: Local RNNoise neural denoising followed by voice levelling.
- **Audio**: WASAPI loopback and microphone capture, with timestamped WAV/RF64 stems streamed to disk through bounded queues. GUI recordings exclude Cam's own audio through Windows process loopback.
- **Long projects**: Editor audio peaks, enhancement, playback and export use disk-backed assets and bounded blocks. Decoded stereo caches need approximately 384 kB per second per source (about 1.3 GiB per hour), plus enhancement/temporary export files. The writable cache is under `%LOCALAPPDATA%/Cam/cache/audio-v1` (temp fallback), so preview/export also work from read-only projects. Least-recently-used complete assets are trimmed toward 2 GiB on access; open assets are protected and may temporarily exceed that target. Older package-local cache folders are not removed automatically.
- **Speech-to-Text**: Embedded `whisper.cpp` (`whisper-rs`) for local transcription. Audio streams into bounded 16 kHz chunks; one loaded model is reused, and word times remain on the original recording timeline. Leaving the editor cancels work; failed/cancelled transcription preserves existing captions. The model itself has a fixed memory cost. `audio-check` also streams its input.

## License

Dual-licensed under MIT or Apache 2.0.

## Windows verification

```pwsh
./scripts/build.ps1
./scripts/selftest.ps1
# Uses the connected microphone, webcam, and a quiet speaker test tone:
./scripts/selftest.ps1 -Hardware -Gui
# Two-minute screen/webcam/audio recording, memory sampling and full media decoding:
./scripts/test-endurance.ps1
# One-hour project export, independent audio checks, native playback/seek and memory controls:
./scripts/test-long-audio.ps1 -Release
# One-hour transcription/diagnostic memory and caption timing check (known 14-word WAV):
./scripts/test-long-transcription.ps1 -Release -Speech path/to/speech.wav
```

The media interoperability tests check codec/container metadata with ffprobe, decode changing video frames, and verify exported audio. Live checks additionally cover native controls, shortcut conflicts, WGC area capture, pause/resume, microphone/webcam files, loopback audio, voice-over, and export. Legacy .sh/osascript scripts are upstream macOS workflows; use the PowerShell scripts on Windows.

Additional component and runtime licensing notes are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

The optional ignored hardware/model suite requires the Whisper base model to have been downloaded once through Captions or `cam transcribe`. It includes actual inference cancellation as well as connected audio/camera/GPU and native keyboard checks.

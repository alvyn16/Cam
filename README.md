<img src="docs/screenshots/icon.png" width="96" alt="Cam icon">

# Cam

**A fast, native Windows screen recorder and video studio, built in Rust.**

Cam is a high-performance, native Rust screen recording and video editing application tailored specifically for Windows. Record your entire display, a specific window, or a cropped area with smooth frame rates. Cam automatically follows mouse movement, generates spring-animated zooms on clicks, re-renders high-DPI cursors, frames recordings with customizable backgrounds, padding, and rounded corners, and exports polished MP4 videos and GIFs.

![Editor](docs/screenshots/editor.png)

## Features

- **Native Windows Capture**:
  - Windows Graphics Capture (WGC) and DXGI Desktop Duplication for ultra-low latency, hardware-accelerated capture.
  - Record full displays, individual windows (tracked front-to-back), or custom regions.
  - Multi-track audio: WASAPI system audio loopback and microphone stems recorded separately.
  - Webcam capture with live, draggable camera bubble overlay.
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
- **Hardware-Accelerated Export**:
  - MP4 video export (H.264 / HEVC) and high-quality delta-frame GIFs.
  - Presets from 720p to 4K, 30 to 60 fps, with file-size targeting.
  - "Export & Copy" to place finished videos directly on the Windows clipboard.

![Recording setup with live preview](docs/screenshots/home.png)
![Editor light mode](docs/screenshots/editor-light.png)

## Getting Started

### Prerequisites

- **OS**: Windows 10 (version 1903+) or Windows 11
- **Toolchain**: Rust 2024 edition (`rustup update stable`)
- **DirectX**: DirectX 11 / 12 capable GPU

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
      mouse.json        # High-rate mouse position & click events
      keys.json         # Keystrokes
      transcript.json   # Generated Whisper captions
  ```

## Architecture

- **GUI**: Built with `egui` rendered through `wgpu` (DirectX 12 / Vulkan backend) on top of `winit`.
- **Capture**: Windows Graphics Capture API and DXGI desktop replication integrated directly through native `windows-rs` bindings.
- **Audio**: WASAPI loopback capture and low-latency audio stream processing.
- **Speech-to-Text**: Embedded `whisper.cpp` (`whisper-rs`) for fully local, private AI transcription.

## License

Dual-licensed under MIT or Apache 2.0.

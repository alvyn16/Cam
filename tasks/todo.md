# Cam implementation checklist

- [x] Fix rapid playhead dragging freezes: background latest-request decoding, obsolete seek cancellation, bounded preview buffers, correct final frames/camera tail and explicit worker shutdown. Real-project and opening-seconds native timeline checks passed with zero message timeouts; details in IMPLEMENTATION_STATUS.md.

- [x] Real MP4/MOV encoding, full-frame decoding, compressed audio/resampling; external ffprobe and changing-frame tests.
- [x] Correct monitor IDs/dimensions/origins and window PID/position/z-order; area and multi-monitor regression checks, including full picker coverage at 100% and 125% DPI scaling.
- [x] Real display/window/area/pattern recording, pause/resume, cancel, graceful Ctrl-C; independently decode live recording.
- [x] Live source preview and system/microphone levels; validate frame delivery.
- [x] Separate system/microphone tracks, synchronized capture and voice-over; audible tone round trip.
- [x] Camera discovery/recording/preview and device source handling; connected webcam validated. Direct USB iPhone capture is a remaining platform gap; Windows driver prerequisites are documented.
- [x] Windows hotkeys, floating bar, tray, keyboard/cursor logging; native controls/conflicts checked.
- [x] Clipboard file payload, privacy reporting, resize error handling, hide icons/taskbar lifecycle implemented.
- [x] File-size targeting, voice enhancement and hardware acceleration checked. Native compositor-to-hardware-encoder interop is implemented; source GPU decoding/capture remain separate parity work.
- [x] Final Windows regression/release checks: 142 standard tests, 14 explicit native audio/GPU/Whisper tests and four live integration tests passed. Current optimized GUI recording/export, read-only preview/export and console cancellation passed. Exact build hash and remaining platform differences are in IMPLEMENTATION_STATUS.md.

## Remaining work toward the full goal

- [x] Replace full-recording audio capture buffers with bounded disk streaming; verify timestamp gaps, pauses, cancellation, overload recovery and live endurance. Independent full decoding passed for all four tracks of a real 120-second recording, with 7.09 MiB private-memory variation after warmup.
- [x] Implement the ignored `exclude_own_audio` option. Native process-excluding loopback is wired through an explicit audio-source enum. An actual recorded stem excluded all of the 1.2-second own-process tone while retaining 2.15 seconds of the external-process tone.
- [x] Honor capture codec/quality options in both writers. Independent screen/camera capture probes confirm H.264, HEVC and ProRes; a real recording confirms HEVC/NVENC screen video and H.264 webcam video.
- [x] Remove unbounded editor/preview/export audio buffering. The 3600-second project passed output audio checks at start/middle/end, native mute/volume and audible seek to 57 minutes. Audio export/editor memory stayed within 128 MiB of controls (measured differences -6.31/-0.60 MiB). This low-resolution fixture does not prove high-resolution GPU performance.
- [x] Audit full-track memory in transcription and CLI audio diagnostics; handle read-only package caches and cache disk usage. Hour-long speech/tone analysis and ACL-denied-write package checks passed. Cache ownership, sidecar leases, concurrent publication/repair and eviction have focused regression coverage.
- [x] Native GPU compositor-to-encoder export. DX12/D3D11 shared surfaces, GPU BGRA-to-NV12 conversion and native NVIDIA H.264/HEVC encoding passed independent pixel/handle/media checks. Production worker export passed audio muxing, cancellation, quality, size fitting, repeated/sustained resource checks and final real GUI acceptance.
- [ ] Source GPU decoding and capture without CPU transfers remain performance differences. Current CPU-transfer paths work; full GPU media interoperability is not claimed. See tasks/gpu-capability-map.md.
- [ ] Implement/verify direct USB iPhone/iPad capture. No iPhone/iPad is currently available for live checks; driver-exposed DirectShow video alone is not full parity.
- [x] Re-audit upstream feature scope and final current app; repeat standard, hardware and release checks after cache/input/console fixes. Remaining platform differences are explicit in GLIDE_COMPARISON.md.

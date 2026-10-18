# Responsive timeline scrubbing

Reported problem: dragging the blue playhead quickly causes the editor to freeze or move slowly. Baseline on the user's recent 1920x1080 recording: two-frame FFmpeg seeks took 110.72-384.41 ms each. The interactive Preview called FrameSource::frame_at, decoder process teardown/startup and blocking raw-video reads directly on the UI thread. Forward jumps could also decode many intervening frames synchronously. While playing, each seek unnecessarily restarted audio output.

Requirements:
- UI seek/render requests perform no decoder process work or blocking pipe reads. Keep one pending target and one completed reply, replacing superseded requests.
- Cancel obsolete backward/large seeks, kill/reap decoder children and join pipe readers. Normal small forward playback must make progress instead of being cancelled on every animation frame.
- Bound decoded preview dimensions and queues; full-resolution synchronous offline rendering/export stays unchanged. Screen and camera requests preserve source mapping and camera offsets.
- After dragging stops, render the final requested frame. Stale completed work must never replace a newer discontinuous seek. Poll/repaint while waiting, without keeping a queue of mouse positions.
- Seeking while already playing only moves the audio clock; normal Play still starts output.
- Verify final screen/camera pixels, rapid request submission, continuous playback, cancellation/teardown, real-project seek latency and final-frame settling. Keep screenshots/throughput benchmarks synchronous so they still check complete rendered frames.

Accepted 2026-10-18 on optimized SHA-256 C5CD7753E76755236385BCB12830BCACD48ECABDCCEDC258BD3A3032B387EE7F. Final renderer p95 UI time was 0.423 ms, worst 1.526 ms, and final GPU/image settling 373.7 ms. Full-project and opening-seconds native window-message checks had no timeouts, with worst round-trip response 10.8 ms. Final library refresh: 127 passing checks. Exact artifacts and measurement limits are in IMPLEMENTATION_STATUS.md. Diagnostic logging is buffered off the UI thread and joined on exit; its earlier synchronous form was unsuitable for native timing.

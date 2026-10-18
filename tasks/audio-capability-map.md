# Bounded audio processing

The user authorized completing the Windows port and keeping all existing features working. Preserve the current audible behavior while removing full-duration audio allocations from production editor/export paths. The previous bounded capture slice is already implemented and verified; this map covers the remaining post-capture pipeline.

| Module id | Responsibility | Depends on |
|---|---|---|
| audio-assets | Decode standard media to atomically published disk-backed 48 kHz stereo assets; bounded random-access cursors and peak scanning | Existing FFmpeg process boundary |
| voice-cache | Stateful RNNoise/DSP and levelling to disk-backed voice caches, preserving existing algorithms | audio-assets |
| timeline-audio | Render bounded timeline blocks with cuts, speed, gains, voice-over placement/ducking, music/fades and click sounds | audio-assets, voice-cache |
| export-audio | Produce a streamed soundtrack and mux it without materializing the entire mix in RAM | timeline-audio |
| preview-audio | Prefetch bounded playback blocks, preserve live master volume/limiting, seeking and device recovery | timeline-audio |

Build order: audio-assets → voice-cache → timeline-audio → export-audio → preview-audio.

Each module retains cancellation/error handling and owns only its temporary/cache files. A source asset is immutable once published. The renderer, recording packages, timeline model, and export formats remain compatible. Existing full-buffer helpers may remain as short-fixture reference APIs, but production long-project paths must use the bounded implementation.

Final acceptance: an actual hour-long media/project fixture can be opened, sought near its end, edited and exported with audio. Audio-processing private-memory growth must remain below 128 MiB beyond the video/GPU baseline for the same output resolution. Independently decode start/middle/end audio, compare short feature-rich mixes against the existing mixer, verify stale jobs cancel, and verify all live playback/mute/volume/seek checks still pass. All five modules now have bounded production implementations and focused reference/error/cancellation tests. scripts/test-long-audio.ps1 passed for a 3600-second/1-fps fixture: memory differences -6.31 MiB export and -0.60 MiB editor; independent sections 0/1800/3597 seconds passed; audible seek to 3420 seconds, live volume/mute and max drift 25 ms passed. Evidence: %TEMP%/cam-long-audio-7dcaffca6a9d4c6e83ad87a3aa7c4d00. This closes this pipeline slice, not the full upstream parity goal.

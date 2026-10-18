# Spec: preview-audio

Replace the editor's full-track audio allocation with BlockMixer preparation and a bounded playback window. Decode and enhance sources in cancellable background jobs. A single producer owns the active graph and renders at most four 4096-frame blocks plus an interpolation frame around the current playhead. The device callback performs no file I/O or decoding and keeps its existing live master volume, meters, drift correction and output recovery.

Publish only the newest completed graph. Keep an existing graph during preparation, report preparation/render failures to the editor, and distinguish pending preparation from a published first window. Seeking backward, forward, across window boundaries and beyond EOF must yield the same samples as the reference mixer. Muting all sources publishes no audio. Close and application exit cancel preparation and join workers before Windows teardown. Memory for audio samples must stay independent of project duration.

Verify bounded-window sampling against the short reference mix, source errors, rapid replacement, mute/unmute and shutdown. Run tests/audio.rs, the native playback self-test and the full suite. Final acceptance also requires the hour-long editor/export memory and seek checks described in audio-capability-map.md. This slice does not close the remaining upstream parity gaps.

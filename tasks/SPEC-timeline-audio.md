# Spec: timeline-audio

## Objective and contract

Implement the timeline-audio module in audio-capability-map.md: render at most 4096 stereo frames per call from immutable disk-backed sources. Source sample buffers and scratch buffers must not scale with playback duration. The API accepts a project, recording, TimeMap, optional package directory, controlled cache directory and cancellation predicate; it returns None only when no audio source/effect is enabled. An enabled missing/corrupt source is an explicit error.

Preserve source cuts, source-to-playback speed mapping and interpolation, per-slice system/mic gains, system/mic muting, enhanced microphone/voice-over caches, voice-over placement at normal speed, overlapping narration, 150 ms duck ramps, music looping and final one-second fade, and click sound timing/volume. Keep the original addition order and optional master volume/soft limiter. Playback uses unmastered blocks so its master volume remains live.

Honor the original frame rounding at slice/voice/click boundaries. Arbitrary forward/backward playback seeks must work without rebuilding the entire mix. Zero-pad beyond sources/playback. Cancellation is checked at source preparation and block boundaries; invalid durations and oversized render blocks are rejected.

## Structure, commands and style

Implementation: src/export/block_mix.rs. Use AudioFile/AudioCursor and voice_cache; keep the original full-buffer mixer as a short-fixture reference. Reuse the click waveform/limiter primitives rather than inventing a different sound. Store track/event/slice metadata; never build full-length output, voice or duck-envelope vectors.

Focused tests: `cargo test -j1 --lib export::block_mix::tests -- --nocapture`

Regression: `cargo test -j1 --all-targets --no-fail-fast`

Build: `cargo build -j1`

Use existing Rust Result conventions, fixed block bounds, u64 frame offsets, descriptive path context, and formatting limited to touched files.

## Verification and boundaries

Compare concatenated and randomly sought blocks against mix/mix_unmastered on feature-rich short fixtures with all tracks/effects enabled, fractional slice boundaries and speed changes, non-aligned block ends, narration overlap, music looping/fade, and click boundaries. Compare both enhancement settings and mute/master controls. Keep absolute sample error below 2e-6 after the shared RMS precision fix.

Verify no-audio behavior, missing-source errors, zero padding, cancellation and oversized-block rejection. The hour-long memory/workflow acceptance test is required after export/preview integration; passing this provider does not complete the full goal.

Always preserve the project/recording schemas, source files and existing audible features. No publication, telemetry, system-memory changes, or unrelated deletion. Full-buffer helpers remain reference/backward-compatible APIs; production long-project consumers must migrate to this provider.

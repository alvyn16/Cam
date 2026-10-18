# Spec: voice-cache

## Objective and contract

Implement the voice-cache provider in audio-capability-map.md. Enhance microphone and voice-over assets without full-track sample/gain allocations. Preserve the existing RNNoise, global voiced RMS levelling and 50 ms AGC/interpolation behavior; do not reset neural or envelope state between disk blocks. Match the existing clean_voice reference on feature-rich short inputs within 2e-6 absolute sample error.

The provider accepts an immutable AudioFile and cancellation predicate and returns a complete immutable AudioFile with the same frame count. Cache identity derives from the decoded source identity and a versioned enhancement algorithm. Cache hits avoid recomputation. A changed source or algorithm cannot reuse stale enhanced samples.

The old global float32 energy sum measurably changed gain with track length (constant 0.1 input produced first samples 0.12497964 vs 0.12581208 for one-second vs twenty-second inputs). Use float64 accumulation in both reference and streaming levelling, preserving the same RMS target, floor and gain cap while removing this precision defect and the reference's extra voiced-sample vector.

Denoise to a uniquely owned intermediate file while accumulating voiced energy/count in constant memory. Compensate RNNoise's one-frame latency with the same padded flush as the reference. Compute the existing global gain, then stream levelling/AGC with a bounded two-window lookahead rather than a full gain vector. Atomically publish only completed output; clean intermediate/pending files on success, cancellation and failure. No caller's source or cache entry may be deleted by a different job.

## Commands and structure

Implementation: src/export/voice_cache.rs; register in src/export/mod.rs. Use src/media/audio_file.rs for immutable cursors and transactional writers. Keep the old buffered enhancement as an independent short-fixture reference.

Focused tests: `cargo test -j1 --lib export::voice_cache::tests -- --nocapture`

Regression: `cargo test -j1 --all-targets --no-fail-fast`

Build: `cargo build -j1`

Use existing Rust Result/cancellation conventions. Fixed-size RNNoise arrays and two 2400-frame stereo buffers are appropriate; no sample or gain collection may scale with duration. Cache source samples are float32; do not add intermediate quantization.

## Verification and boundaries

Compare against clean_voice on silence, varying speech/noise amplitudes, stereo input, non-multiple-of-480 tails and inputs shorter than an AGC window. Check identical duration and preserved state across windows. Verify cache hits, source identity changes, concurrent jobs, cancellation during denoising and during levelling, and intermediate-file cleanup.

Always preserve source files, current enhancement behavior, cancellation and explicit errors. Continue within the authorized local implementation scope; no push, publication, telemetry or unrelated deletion. This provider alone does not complete bounded preview/export or the full goal.

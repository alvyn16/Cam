# Spec: audio-assets

## Objective

Provide immutable, disk-backed decoded audio assets and small random-access cursors for the remaining bounded audio pipeline. Avoid allocating a Vec proportional to media duration. Preserve 48 kHz stereo float samples produced by the existing FFmpeg decoder, including imported compressed formats, channel conversion and resampling.

Assumptions: Windows 11 remains the verified platform; FFmpeg/ffprobe are installed or configured through the existing overrides; package-local cache files are permitted. Preserve current package/timeline schemas and original source files. No user confirmation is needed for these reversible implementation changes under the active request to finish the app.

## Contract

- A decoded asset has a known u64 stereo-frame count, a stable owned path and immutable float data.
- Decode to a unique staging file, then atomically publish a complete cache entry. The cache key includes the canonical source path, source size/modified time, selected audio stream, sample format/rate and cache version. Changed sources cannot reuse stale samples.
- Decode directly to disk through FFmpeg. Check subprocess status and exact frame alignment. Empty/missing/non-audio streams produce explicit errors. Cancelled or failed jobs remove their own staging files and terminate decoder processes.
- A cursor owns a bounded window of decoded samples and offers zero-padded, linearly interpolated sampling compatible with export::audio::sample. It supports arbitrary forward/backward seeks without reopening or fully reading the asset.
- Cursor memory is fixed by its window size, not by track duration. Asset metadata is O(1); sample data stays on disk.
- Peak scanning reads fixed-size blocks. It cannot route through media::read_audio or allocate the entire track.
- Multiple readers may share one immutable published asset. Concurrent decoding must not expose partial output or remove another job's staging files.

## Commands

Build: `cargo build -j1`

Focused tests: `cargo test -j1 --lib media::audio_file::tests -- --nocapture`

Regression: `cargo test -j1 --all-targets --no-fail-fast`

Independent checks use the existing `media::ffmpeg::command` with ffprobe and FFmpeg. Do not compare only the application's own reader and writer.

## Structure and style

Implementation: src/media/audio_file.rs, exported from src/media/mod.rs. Keep FFmpeg location/hidden-process policy in src/media/ffmpeg.rs. Tests belong alongside the provider; consumer integration tests stay in tests/.

Use Rust Result with path/context for media/file errors and u64 frame offsets. Example intent: `let mut cursor = asset.cursor()?; let sample = cursor.sample_at(frame)?;`. Keep platform byte order and stereo-frame alignment explicit. Format only touched files with rustfmt and skip_children=true; do not reformat unrelated modules.

## Verification

- Decode stereo WAV, mono compressed audio, and a non-48-kHz fixture. Compare bounded-window samples against independent FFmpeg f32 output at beginning, window boundaries, arbitrary middle/end seeks, and out-of-range positions.
- Verify interpolation matches the reference mixer, including the final frame and negative/fractional positions.
- Modify a source and prove cache invalidation. Select different audio streams and prove distinct assets. Missing/invalid streams and cancellation must leave no published partial cache.
- Run simultaneous readers/decoders against one source and ensure complete consistent output.
- Scan peaks with a deliberately large asset and inspect the code/allocation bounds. The final hour-long private-memory measurement belongs to the integrated pipeline, not a narrow unit test.

## Boundaries

Always preserve source files, package compatibility, audio precision, cancellation, and explicit I/O/process errors. Keep disk writes in controlled package caches or uniquely owned temporary directories.

Continue within the user's authorization for ordinary dependency/implementation choices. Do not push, publish, modify system memory settings, add telemetry, or delete unrelated cache/source files. Never replace a failing source with silence and report success. Do not declare the full goal complete based on this provider alone.

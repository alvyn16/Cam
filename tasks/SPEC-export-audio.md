# Spec: export-audio

## Objective and contract

Implement export-audio from audio-capability-map.md. Render the complete mastered soundtrack through BlockMixer into a uniquely owned WAV/RF64 file using fixed-size blocks. Mux from that file without loading it into a Vec. Keep the existing H.264/HEVC/ProRes video and AAC audio behavior, file-size fitting, and preservation of an existing destination when cancellation/failure occurs.

The rendered soundtrack has exactly the timeline's frame count. No-audio projects remain video-only. Missing enabled audio sources fail explicitly. Cancellation stops decoding, enhancement, block rendering, encoder flushing and muxing, and cleans only this job's temporary audio/video files. Existing sample-buffer MovieWriter callers remain supported for short fixtures and API compatibility; production export must use file-backed soundtracks.

## Structure, commands and style

Add src/export/stream_audio.rs for transactional, owned temporary soundtracks. Reuse media::wav::WaveWriter (including RF64 size support), BlockMixer and existing FFmpeg process policy. Extend media::writer with a file-soundtrack variant and cancellable finalization; connect export/mod.rs to it. Do not change project schemas or audio features.

Focused tests: `cargo test -j1 --lib export::stream_audio::tests -- --nocapture`

Interoperability: `cargo test -j1 --test media_interop --test audio`

Regression: `cargo test -j1 --all-targets --no-fail-fast`

Build: `cargo build -j1`

Use Result with cancellation/path context, owned-file guards, bounded decoder/error-log buffers and formatting limited to touched files.

## Verification and boundaries

Independently decode the rendered WAV and final exports, compare short soundtrack samples with the reference mix, and verify tone duration/volume in the final AAC stream. Test cancellation before/during audio rendering and during finalization, missing inputs, no-audio output, temporary cleanup and preservation of existing destinations/imported soundtracks. Keep codec and file-size regression coverage.

The final hour-long private-memory and editor/export workflow check remains required after preview integration. No publication, unrelated deletion, source mutation, telemetry, or system-memory changes. This slice is not completion of the full goal.

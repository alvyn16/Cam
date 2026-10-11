//! Capture: ScreenCaptureKit screen + audio, AVFoundation camera, and the
//! constant-frame-rate AVAssetWriter sink.
//!
//! Adapted from rec-rs by Michael Shimeles (MIT OR Apache-2.0), the same
//! author's recorder; see README for credit.

pub mod camera;
pub mod compose;
pub mod mixer;
pub mod pattern;
pub mod record;
pub mod screen;
pub mod tcc;
pub mod writer;

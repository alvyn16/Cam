//! Glide: a fast, native macOS screen recorder and editor.
//!
//! Behaviour was reverse engineered from Screen Studio with REA (see
//! `analysis/SCREEN_STUDIO.md`); all code here is written independently.
//! The capture pipeline builds on rec-rs by Michael Shimeles.

pub mod engine;
pub mod project;
pub mod render;
pub mod synth;

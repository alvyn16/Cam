//! Voice isolation: run a microphone track through voice enhancement.
//! Removes background noise; returns None when unavailable.

use super::audio::Stereo;

pub fn isolate_voice(input: &[Stereo]) -> Option<Vec<Stereo>> {
    let _ = input;
    None
}

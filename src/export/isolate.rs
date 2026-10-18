//! RNNoise neural voice isolation, with its built-in model (BSD-3-Clause).
use super::audio::Stereo;
pub fn isolate_voice(input: &[Stereo]) -> Option<Vec<Stereo>> {
    if input.is_empty() {
        return Some(vec![]);
    }
    const N: usize = nnnoiseless::DenoiseState::FRAME_SIZE;
    let mut state = nnnoiseless::DenoiseState::new();
    let mut result = Vec::with_capacity(input.len() + N);
    let mut block = [0.0f32; N];
    let mut out = [0.0f32; N];
    for i in 0..=input.len().div_ceil(N) {
        block.fill(0.0);
        for (dst, s) in block
            .iter_mut()
            .zip(input.get(i * N..).unwrap_or(&[]).iter().take(N))
        {
            *dst = (s[0] + s[1]) * 0.5 * 32768.0;
        }
        state.process_frame(&mut out, &block);
        if i > 0 {
            result.extend(out.iter().map(|s| {
                let v = (*s / 32768.0).clamp(-1.0, 1.0);
                [v, v]
            }));
        }
    }
    result.truncate(input.len());
    Some(result)
}
#[cfg(test)]
mod tests {
    #[test]
    fn denoising_preserves_duration_and_reduces_stationary_noise() {
        let mut seed = 42u32;
        let input: Vec<_> = (0..96000)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let v = (seed as f32 / u32::MAX as f32 - 0.5) * 0.02;
                [v, v]
            })
            .collect();
        let output = super::isolate_voice(&input).unwrap();
        assert_eq!(input.len(), output.len());
        let energy = |v: &[[f32; 2]]| v.iter().skip(48000).map(|s| s[0] * s[0]).sum::<f32>();
        assert!(
            energy(&output) < energy(&input) * 0.9,
            "energy ratio {}",
            energy(&output) / energy(&input)
        );
    }
}

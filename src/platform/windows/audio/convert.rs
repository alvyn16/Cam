//! Device-clocked, packet-independent conversion to the recording format.
//! Callback scheduling must never decide where individual audio packets go.
use std::collections::VecDeque;

const RATE: u64 = 48_000;
const TAPS: usize = 32;
const HALF: i64 = TAPS as i64 / 2;
const PHASES: usize = 1024;

pub(super) struct Converter {
    rate: u32,
    channels: usize,
    origin: Option<f64>,
    input: u64,
    output: u64,
    history: VecDeque<[f32; 2]>,
    base: i64,
    kernel: Vec<[f32; TAPS]>,
}
impl Converter {
    pub fn new(rate: u32, channels: usize) -> Self {
        assert!(rate > 0 && channels > 0);
        let cutoff = (RATE as f64 / rate as f64).min(1.0) * 0.94;
        let kernel = if rate == RATE as u32 {
            Vec::new()
        } else {
            (0..=PHASES)
                .map(|phase| {
                    let fraction = phase as f64 / PHASES as f64;
                    let mut taps = [0.0; TAPS];
                    for (i, tap) in taps.iter_mut().enumerate() {
                        let distance = i as f64 - (HALF - 1) as f64 - fraction;
                        let x = std::f64::consts::PI * distance * cutoff;
                        let sinc = if x.abs() < 1e-12 { 1.0 } else { x.sin() / x };
                        let window = 0.42
                            + 0.5 * (std::f64::consts::PI * distance / HALF as f64).cos()
                            + 0.08 * (2.0 * std::f64::consts::PI * distance / HALF as f64).cos();
                        *tap = (sinc * cutoff * window) as f32;
                    }
                    let sum: f32 = taps.iter().sum();
                    for tap in &mut taps {
                        *tap /= sum;
                    }
                    taps
                })
                .collect()
        };
        Self {
            rate,
            channels,
            origin: None,
            input: 0,
            output: 0,
            history: VecDeque::new(),
            base: 0,
            kernel,
        }
    }

    /// `capture` is the hardware timestamp of the first input sample, never
    /// callback arrival time. Small device-clock quantization is snapped to the
    /// sample count; real missing packets retain their gap on the video clock.
    pub fn push(&mut self, data: &[f32], capture: f64) -> (f64, Vec<f32>) {
        let discontinuity = self.origin.is_some_and(|origin| {
            (capture - (origin + self.input as f64 / self.rate as f64)).abs() > 0.002
        });
        if self.origin.is_none() || discontinuity {
            self.origin = Some(capture);
            self.input = 0;
            self.output = 0;
            self.base = 0;
            self.history.clear();
        }
        let at = self.origin.unwrap() + self.output as f64 / RATE as f64;
        let stereo = data.chunks_exact(self.channels).map(|frame| {
            let finite = |v: f32| if v.is_finite() { v } else { 0.0 };
            [finite(frame[0]), finite(*frame.get(1).unwrap_or(&frame[0]))]
        });
        if self.rate == RATE as u32 {
            let mut result = Vec::with_capacity(data.len() / self.channels * 2);
            for frame in stereo {
                result.extend_from_slice(&frame);
            }
            self.input += (result.len() / 2) as u64;
            self.output = self.input;
            return (at, result);
        }
        self.history.extend(stereo);
        self.input = self.base as u64 + self.history.len() as u64;
        let mut result = Vec::with_capacity(
            data.len() / self.channels * 2 * RATE as usize / self.rate as usize + 2,
        );
        loop {
            // Integer phase accumulation avoids packet rounding and long-run drift.
            let position = self.output * self.rate as u64;
            let center = (position / RATE) as i64;
            if center + HALF >= self.input as i64 {
                break;
            }
            let phase = ((position % RATE) * PHASES as u64 / RATE) as usize;
            let mut sample = [0.0; 2];
            for (i, weight) in self.kernel[phase].iter().enumerate() {
                let index = center + i as i64 - (HALF - 1);
                let frame = self.history[(index.max(0) - self.base) as usize];
                sample[0] += frame[0] * weight;
                sample[1] += frame[1] * weight;
            }
            result.extend_from_slice(&sample);
            self.output += 1;
        }
        let keep = ((self.output * self.rate as u64 / RATE) as i64 - HALF).max(0);
        while self.base < keep {
            self.history.pop_front();
            self.base += 1;
        }
        (at, result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn signal(rate: u32, seconds: usize, hz: f64) -> Vec<f32> {
        (0..rate as usize * seconds)
            .map(|i| (std::f64::consts::TAU * hz * i as f64 / rate as f64).sin() as f32 * 0.3)
            .collect()
    }
    #[test]
    fn irregular_packets_match_one_continuous_resampling_and_have_no_timeline_holes() {
        for rate in [16_000, 44_100, 48_000, 96_000] {
            let source = signal(rate, 2, 1234.0);
            let (_, reference) = Converter::new(rate, 1).push(&source, 10.0);
            let mut converter = Converter::new(rate, 1);
            let mut actual = Vec::new();
            let mut input = 0;
            for packet in source.chunks(137) {
                let (at, samples) = converter.push(packet, 10.0 + input as f64 / rate as f64);
                assert!(((at - 10.0) * RATE as f64 - actual.len() as f64 / 2.0).abs() < 1e-6);
                actual.extend(samples);
                input += packet.len();
            }
            assert_eq!(actual, reference, "packet-dependent output at {rate}");
            assert!((actual.len() as i64 / 2 - 96_000).abs() <= 49);
            assert!(converter.history.len() <= TAPS + 1);
            let error = actual
                .chunks_exact(2)
                .enumerate()
                .skip(100)
                .map(|(i, s)| {
                    let expected =
                        (std::f64::consts::TAU * 1234.0 * i as f64 / RATE as f64).sin() * 0.3;
                    (s[0] as f64 - expected).powi(2)
                })
                .sum::<f64>()
                / (actual.len() / 2 - 100) as f64;
            assert!(error < 1e-6, "resampling error at {rate}: {error}");
        }
    }
    #[test]
    fn downsampling_rejects_out_of_band_audio() {
        let source = signal(96_000, 1, 32_000.0);
        let (_, samples) = Converter::new(96_000, 1).push(&source, 1.0);
        let rms = (samples
            .iter()
            .skip(200)
            .map(|s| (*s as f64).powi(2))
            .sum::<f64>()
            / (samples.len() - 200) as f64)
            .sqrt();
        assert!(rms < 0.003, "aliased high-frequency tone: {rms}");
    }
    #[test]
    fn native_rate_preserves_stereo_and_real_discontinuities() {
        let mut converter = Converter::new(48_000, 2);
        assert_eq!(
            converter.push(&[0.25, -0.5, 0.75, -0.25], 2.0),
            (2.0, vec![0.25, -0.5, 0.75, -0.25])
        );
        let (at, _) = converter.push(&[0.1, 0.2], 2.0001);
        assert!((at - (2.0 + 2.0 / 48_000.0)).abs() < 1e-9);
        let (at, _) = converter.push(&[0.1, 0.2], 3.0);
        assert_eq!(at, 3.0);
        assert_eq!(
            converter.push(&[f32::NAN, f32::INFINITY], 4.0).1,
            vec![0.0, 0.0]
        );
    }
}

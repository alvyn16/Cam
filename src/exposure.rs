//! "Data may be visible": a blur or pixelate mask hides whatever was under
//! it when it started. If that content moves (a page scrolls, a list
//! re-sorts) the private part can slide out from under the mask. This
//! samples the recording under each hiding mask every 100 ms and reports
//! the ranges where it no longer looks like the first frame, so the editor
//! can warn and offer to cut those parts.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;

use crate::engine::geom::Rect;
use crate::media::FrameSource;
use crate::project::model::{Mask, MaskKind};

/// Time between compared frames.
pub const SAMPLE_MS: f64 = 100.0;
/// Below this structural similarity (mean SSIM) the content changed.
pub const MIN_SIMILARITY: f64 = 0.88;
/// Below this brightness-histogram overlap the content changed.
pub const MIN_HISTOGRAM: f64 = 0.85;
/// Changed samples closer than this form one range.
pub const MERGE_GAP_MS: f64 = 200.0;
/// Extra time cut on each side of a flagged range.
pub const CUT_PAD_MS: f64 = 100.0;

/// Compared patches are this many pixels square.
const SIDE: usize = 64;
const BINS: usize = 32;

/// A range (source ms) where the content under a mask moved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Exposure {
    pub mask: u64,
    pub start: f64,
    pub end: f64,
}

/// Masks that hide content (a highlight shows it).
pub fn hides(kind: MaskKind) -> bool {
    matches!(kind, MaskKind::Blur | MaskKind::Pixelate)
}

/// The masked region scaled to SIDE×SIDE brightness values.
pub struct Patch {
    luma: Vec<f32>,
}

impl Patch {
    /// Sample `rect` (normalised to the frame) from BGRA pixels.
    pub fn from_bgra(bgra: &[u8], width: u32, height: u32, stride: usize, rect: Rect) -> Option<Patch> {
        let (w, h) = (width as f64, height as f64);
        let x0 = (rect.x * w).clamp(0.0, w);
        let y0 = (rect.y * h).clamp(0.0, h);
        let x1 = ((rect.x + rect.w) * w).clamp(0.0, w);
        let y1 = ((rect.y + rect.h) * h).clamp(0.0, h);
        if x1 - x0 < 1.0 || y1 - y0 < 1.0 || bgra.len() < stride * height as usize {
            return None;
        }
        // Average a 4×4 grid of taps per cell: enough to smooth compression
        // noise without reading every pixel of a large 5K region.
        const TAPS: usize = 4;
        let (cw, ch) = ((x1 - x0) / SIDE as f64, (y1 - y0) / SIDE as f64);
        let mut luma = vec![0f32; SIDE * SIDE];
        for cy in 0..SIDE {
            for cx in 0..SIDE {
                let mut sum = 0.0f32;
                for ty in 0..TAPS {
                    let y = (y0 + (cy as f64 + (ty as f64 + 0.5) / TAPS as f64) * ch) as usize;
                    let row = y.min(height as usize - 1) * stride;
                    for tx in 0..TAPS {
                        let x = (x0 + (cx as f64 + (tx as f64 + 0.5) / TAPS as f64) * cw) as usize;
                        let i = row + x.min(width as usize - 1) * 4;
                        sum += 0.114 * bgra[i] as f32 + 0.587 * bgra[i + 1] as f32 + 0.299 * bgra[i + 2] as f32;
                    }
                }
                luma[cy * SIDE + cx] = sum / (TAPS * TAPS) as f32;
            }
        }
        Some(Patch { luma })
    }
}

/// Mean SSIM over 8×8 blocks (1 = identical).
pub fn similarity(a: &Patch, b: &Patch) -> f64 {
    const B: usize = 8;
    const C1: f64 = 6.5025; // (0.01·255)²
    const C2: f64 = 58.5225; // (0.03·255)²
    let mut total = 0.0;
    for by in (0..SIDE).step_by(B) {
        for bx in (0..SIDE).step_by(B) {
            let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for y in by..by + B {
                for x in bx..bx + B {
                    let (p, q) = (a.luma[y * SIDE + x] as f64, b.luma[y * SIDE + x] as f64);
                    sa += p;
                    sb += q;
                    saa += p * p;
                    sbb += q * q;
                    sab += p * q;
                }
            }
            let n = (B * B) as f64;
            let (ma, mb) = (sa / n, sb / n);
            let (va, vb, cov) = (saa / n - ma * ma, sbb / n - mb * mb, sab / n - ma * mb);
            total += ((2.0 * ma * mb + C1) * (2.0 * cov + C2)) / ((ma * ma + mb * mb + C1) * (va + vb + C2));
        }
    }
    total / ((SIDE / B) * (SIDE / B)) as f64
}

/// Overlap of the two brightness histograms (1 = identical). Values are
/// split between neighbouring bins so noise near a bin edge doesn't count.
pub fn histogram_overlap(a: &Patch, b: &Patch) -> f64 {
    let hist = |p: &Patch| {
        let mut h = [0f64; BINS];
        for &v in &p.luma {
            let pos = (v as f64 / 256.0 * BINS as f64 - 0.5).clamp(0.0, (BINS - 1) as f64);
            let i = pos.floor() as usize;
            let f = pos - i as f64;
            h[i] += 1.0 - f;
            if f > 0.0 {
                h[i + 1] += f;
            }
        }
        h
    };
    let (ha, hb) = (hist(a), hist(b));
    ha.iter().zip(&hb).map(|(x, y)| x.min(*y)).sum::<f64>() / (SIDE * SIDE) as f64
}

/// Does `sample` still show the content of `reference`?
pub fn matches(reference: &Patch, sample: &Patch) -> bool {
    similarity(reference, sample) >= MIN_SIMILARITY && histogram_overlap(reference, sample) >= MIN_HISTOGRAM
}

/// Merge flagged sample times (ascending) into ranges ending by `end`.
pub fn group(flagged: &[f64], end: f64) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = vec![];
    let mut last = f64::NEG_INFINITY;
    for &t in flagged {
        let stop = (t + SAMPLE_MS).min(end);
        match out.last_mut() {
            Some(r) if t - last <= MERGE_GAP_MS => r.1 = stop,
            _ => out.push((t, stop)),
        }
        last = t;
    }
    out
}

/// What a scan depends on: the mask's area and time range.
type Key = [u64; 6];

fn key(m: &Mask) -> Key {
    [m.rect.x, m.rect.y, m.rect.w, m.rect.h, m.start, m.end].map(f64::to_bits)
}

/// Previous results by mask area and time range.
pub type Cache = HashMap<Key, Vec<(f64, f64)>>;

/// Scan every hiding mask, reusing `cache` for unchanged ones. All masks
/// share one forward decoding pass. `cancelled` is polled between samples.
pub fn scan(video: &Path, masks: &[Mask], cache: &mut Cache, cancelled: &dyn Fn() -> bool) -> Result<Vec<Exposure>> {
    let hiding: Vec<&Mask> = masks.iter().filter(|m| hides(m.kind)).collect();
    let todo: Vec<&Mask> = hiding.iter().copied().filter(|m| !cache.contains_key(&key(m))).collect();
    if !todo.is_empty() {
        // (time, mask index); a mask's reference (its start) sorts first.
        let mut samples = vec![];
        for (i, m) in todo.iter().enumerate() {
            if m.end - m.start < 2.0 * SAMPLE_MS {
                continue;
            }
            let mut t = m.start;
            while t < m.end {
                samples.push((t, i));
                t += SAMPLE_MS;
            }
        }
        samples.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut src = FrameSource::open(video)?;
        let mut reference: Vec<Option<Patch>> = todo.iter().map(|_| None).collect();
        let mut flagged: Vec<Vec<f64>> = vec![vec![]; todo.len()];
        for (t, i) in samples {
            if cancelled() {
                return Ok(vec![]);
            }
            let rect = todo[i].rect;
            let patch = objc2::rc::autoreleasepool(|_| {
                src.frame_at(t).and_then(|f| f.with_bgra(|v| Patch::from_bgra(v.bgra, v.width, v.height, v.stride, rect)))
            });
            let Some(patch) = patch else { continue };
            match &reference[i] {
                None => reference[i] = Some(patch),
                Some(r) if !matches(r, &patch) => flagged[i].push(t),
                Some(_) => {}
            }
        }
        for (i, m) in todo.iter().enumerate() {
            cache.insert(key(m), group(&flagged[i], m.end));
        }
    }
    let mut out: Vec<Exposure> =
        hiding.iter().flat_map(|m| cache[&key(m)].iter().map(|&(start, end)| Exposure { mask: m.id, start, end })).collect();
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 200×100 BGRA frame: light background with dark "text" bars
    /// starting `scroll` px down, plus deterministic noise.
    fn frame(scroll: usize, seed: u32) -> Vec<u8> {
        let (w, h) = (200usize, 100usize);
        let mut x = seed.max(1);
        let mut px = vec![0u8; w * h * 4];
        for y in 0..h {
            for c in 0..w {
                let text = (y + 100 - scroll % 100) % 12 < 5 && (c * 7 + y / 12 * 13) % 40 < 30;
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                let noise = (x % 5) as i32 - 2;
                let v = (if text { 60 } else { 235 } + noise) as u8;
                px[(y * w + c) * 4..][..4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        px
    }

    fn patch(px: &[u8]) -> Patch {
        Patch::from_bgra(px, 200, 100, 800, Rect::new(0.1, 0.1, 0.8, 0.8)).unwrap()
    }

    #[test]
    fn same_content_with_noise_matches() {
        let (a, b) = (patch(&frame(0, 1)), patch(&frame(0, 99)));
        assert!(similarity(&a, &b) > 0.95, "{}", similarity(&a, &b));
        assert!(histogram_overlap(&a, &b) > 0.95);
        assert!(matches(&a, &b));
    }

    #[test]
    fn scrolled_content_is_flagged() {
        let a = patch(&frame(0, 1));
        let b = patch(&frame(6, 2));
        assert!(!matches(&a, &b), "ssim {}", similarity(&a, &b));
    }

    #[test]
    fn brightness_change_is_flagged() {
        let a = patch(&[200u8, 200, 200, 255].repeat(200 * 100));
        let b = patch(&[100u8, 100, 100, 255].repeat(200 * 100));
        assert!(!matches(&a, &b));
        let c = patch(&[201u8, 199, 200, 255].repeat(200 * 100));
        assert!(matches(&a, &c));
    }

    #[test]
    fn grouping_merges_short_gaps() {
        assert_eq!(group(&[100.0, 200.0, 400.0, 900.0], 2000.0), vec![(100.0, 500.0), (900.0, 1000.0)]);
        assert_eq!(group(&[1900.0], 1950.0), vec![(1900.0, 1950.0)]);
        assert!(group(&[], 1000.0).is_empty());
    }

    #[test]
    fn only_hiding_masks_are_checked() {
        assert!(hides(MaskKind::Blur) && hides(MaskKind::Pixelate) && !hides(MaskKind::Highlight));
    }
}

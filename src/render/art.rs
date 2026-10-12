//! Built-in artwork generated in code: wallpapers and vector cursors.
//! (No third-party images are bundled.)

use super::Image;
use crate::engine::geom::{Vec2, v2};

/// Built-in wallpapers: (id, display name, base colour, blobs).
type Blob = ([u8; 3], f32, f32, f32);
pub const WALLPAPERS: [(&str, &str, [u8; 3], [Blob; 4]); 10] = [
    ("aurora", "Aurora", [16, 22, 58], [([60, 214, 190], 0.15, 0.85, 0.7), ([118, 76, 255], 0.85, 0.15, 0.75), ([40, 120, 255], 0.55, 0.65, 0.6), ([255, 120, 200], 0.95, 0.95, 0.45)]),
    ("sunset", "Sunset", [60, 18, 60], [([255, 140, 60], 0.2, 0.9, 0.75), ([255, 70, 120], 0.8, 0.6, 0.7), ([255, 210, 120], 0.5, 1.1, 0.6), ([120, 50, 160], 0.9, 0.05, 0.6)]),
    ("ocean", "Ocean", [6, 30, 60], [([0, 160, 255], 0.1, 0.2, 0.75), ([0, 220, 200], 0.9, 0.9, 0.7), ([30, 90, 200], 0.6, 0.4, 0.6), ([120, 200, 255], 0.3, 0.95, 0.45)]),
    ("mint", "Mint", [210, 245, 230], [([120, 230, 190], 0.1, 0.9, 0.7), ([170, 220, 255], 0.9, 0.1, 0.7), ([250, 255, 220], 0.6, 0.6, 0.6), ([140, 255, 220], 0.95, 0.95, 0.5)]),
    ("grape", "Grape", [28, 10, 48], [([170, 60, 255], 0.2, 0.3, 0.7), ([255, 60, 180], 0.8, 0.8, 0.7), ([80, 40, 200], 0.9, 0.1, 0.6), ([255, 150, 230], 0.1, 0.95, 0.45)]),
    ("dawn", "Dawn", [250, 225, 215], [([255, 180, 160], 0.1, 0.1, 0.8), ([200, 190, 255], 0.9, 0.8, 0.8), ([255, 230, 180], 0.5, 0.95, 0.6), ([255, 200, 230], 0.95, 0.15, 0.55)]),
    ("ember", "Ember", [30, 8, 8], [([255, 90, 30], 0.15, 0.85, 0.7), ([255, 40, 60], 0.85, 0.25, 0.65), ([255, 180, 60], 0.55, 0.55, 0.5), ([120, 20, 40], 0.9, 0.95, 0.6)]),
    ("lagoon", "Lagoon", [10, 50, 50], [([30, 200, 160], 0.2, 0.2, 0.75), ([20, 120, 200], 0.85, 0.75, 0.7), ([160, 255, 210], 0.6, 0.15, 0.5), ([10, 80, 120], 0.1, 0.95, 0.6)]),
    ("graphite", "Graphite", [28, 29, 33], [([70, 72, 82], 0.2, 0.2, 0.8), ([45, 47, 55], 0.85, 0.85, 0.8), ([95, 98, 110], 0.7, 0.2, 0.5), ([20, 20, 24], 0.2, 0.95, 0.6)]),
    ("snow", "Snow", [236, 238, 243], [([255, 255, 255], 0.2, 0.2, 0.8), ([214, 220, 232], 0.85, 0.85, 0.8), ([245, 246, 250], 0.7, 0.2, 0.5), ([225, 230, 240], 0.15, 0.9, 0.6)]),
];

/// Generate a wallpaper: soft colour blobs over a base colour, crossed by
/// translucent "glass" ribbons with crisp edges and a bright rim (so the art
/// has detail a background blur visibly softens), lightly dithered against
/// banding.
pub fn wallpaper(id: &str, w: u32, h: u32) -> Image {
    use std::f32::consts::TAU;
    let index = WALLPAPERS.iter().position(|wp| wp.0 == id).unwrap_or(0);
    let (_, _, base, blobs) = &WALLPAPERS[index];
    // Light wallpapers get darker glass so it still reads.
    let light = base.iter().map(|&c| c as f32).sum::<f32>() > 3.0 * 160.0;
    let (target, glass) = if light { (0.0, 0.16) } else { (255.0, 0.16) };
    let phase = index as f32 * 0.37;
    // Each ribbon's centre line and half-thickness depend only on x.
    let ribbons: Vec<[(f32, f32); 3]> = (0..w)
        .map(|x| {
            let u = x as f32 / w as f32;
            std::array::from_fn(|k| {
                let p = phase + k as f32 * 1.9;
                let centre = 0.22 + 0.28 * k as f32 + 0.09 * (TAU * (0.55 * u + p)).sin() + 0.035 * (TAU * (1.6 * u + 1.3 * p)).sin();
                (centre, 0.055 + 0.025 * (TAU * (0.8 * u + 0.7 * p)).sin())
            })
        })
        .collect();
    let aspect = w as f32 / h as f32;
    let px = h as f32;
    let row = |y: u32, out: &mut [u8]| {
        let v = y as f32 / h as f32;
        let mut seed: u32 = 0x9e37_79b9 ^ y.wrapping_mul(0x85eb_ca6b);
        for (x, o) in out.chunks_exact_mut(4).enumerate() {
            let u = x as f32 / w as f32;
            let mut c = base.map(|c| c as f32);
            for &(col, bx, by, r) in blobs {
                let dx = (u - bx) * aspect;
                let dy = v - by;
                let wgt = (-(dx * dx + dy * dy) / (r * r) * 2.2).exp();
                for i in 0..3 {
                    c[i] += (col[i] as f32 - c[i]) * wgt;
                }
            }
            for &(centre, half) in &ribbons[x] {
                // Coverage of the band, anti-aliased over about a pixel.
                let inside = (0.5 - ((v - centre).abs() - half) * px).clamp(0.0, 1.0);
                // A rim a few pixels wide along the band's upper edge.
                let rim = (1.0 - ((v - (centre - half)) * px - 2.0).abs() / 2.5).clamp(0.0, 1.0);
                if inside > 0.0 || rim > 0.0 {
                    for ch in c.iter_mut() {
                        *ch += (target - *ch) * inside * glass;
                        *ch += (255.0 - *ch) * rim * 0.38;
                    }
                }
            }
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let n = (seed & 0xff) as f32 / 255.0 - 0.5;
            for i in 0..3 {
                o[i] = (c[i] + n * 1.5).clamp(0.0, 255.0) as u8;
            }
            o[3] = 255;
        }
    };
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let stride = (w * 4) as usize;
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16);
    let rows_per = (h as usize).div_ceil(threads).max(1);
    std::thread::scope(|sc| {
        for (i, chunk) in rgba.chunks_mut(rows_per * stride.max(1)).enumerate() {
            let row = &row;
            sc.spawn(move || {
                for (j, line) in chunk.chunks_exact_mut(stride).enumerate() {
                    row((i * rows_per + j) as u32, line);
                }
            });
        }
    });
    Image { width: w, height: h, rgba }
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b - a;
    let t = (((p - a).x * ab.x + (p - a).y * ab.y) / (ab.x * ab.x + ab.y * ab.y)).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

/// Signed distance to a polygon (negative inside).
fn poly_sd(p: Vec2, pts: &[Vec2]) -> f64 {
    let mut d = f64::MAX;
    let mut inside = false;
    let n = pts.len();
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        d = d.min(seg_dist(p, a, b));
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
    }
    if inside { -d } else { d }
}

/// Paint shapes (signed distance fn in shape units) as a cursor: dark fill,
/// white outline, soft drop shadow.
fn paint(units: Vec2, px_per_unit: f64, outline: f64, sd: impl Fn(Vec2) -> f64) -> Image {
    let pad = 4.0;
    let (w, h) = (((units.x + 2.0 * pad) * px_per_unit).ceil() as u32, ((units.y + 2.0 * pad) * px_per_unit).ceil() as u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let p = v2((x as f64 + 0.5) / px_per_unit - pad, (y as f64 + 0.5) / px_per_unit - pad);
            let aa = 0.75 / px_per_unit;
            let d = sd(p);
            let fill = ((-d) / aa + 0.5).clamp(0.0, 1.0);
            let ring = ((outline - d) / aa + 0.5).clamp(0.0, 1.0);
            let ds = sd(p - v2(0.0, 0.9));
            let shadow = (-(ds - outline).max(0.0).powi(2) / 2.5).exp() * 0.35;
            // Composite: shadow, white ring, black fill (premultiplied).
            let mut a = shadow;
            let mut c = 0.0;
            c = c * (1.0 - ring) + ring;
            a = a * (1.0 - ring) + ring;
            c *= 1.0 - fill;
            a = a * (1.0 - fill) + fill;
            let v = (c * 255.0) as u8;
            rgba.extend_from_slice(&[v, v, v, (a * 255.0) as u8]);
        }
    }
    Image { width: w, height: h, rgba }
}

/// A vector cursor image, its hotspot (normalised) and height relative to
/// the standard arrow.
pub fn vector_cursor(kind: Option<&str>) -> (Image, Vec2, f64) {
    const PX: f64 = 6.0;
    let pad = 4.0;
    match kind {
        Some("ibeam") => {
            let units = v2(9.0, 18.0);
            let img = paint(units, PX, 1.2, |p| {
                let bar = (p.x - 4.5).abs().max((p.y - 9.0).abs() - 8.0) - 0.7;
                let top = (p.x - 4.5).abs().max((p.y - 0.6).abs()) - 2.6 + 2.0;
                let bot = (p.x - 4.5).abs().max((p.y - 17.4).abs()) - 2.6 + 2.0;
                bar.min(top.max((p.y - 0.6).abs() - 0.6)).min(bot.max((p.y - 17.4).abs() - 0.6))
            });
            let hot = v2((4.5 + pad) / (units.x + 2.0 * pad), (9.0 + pad) / (units.y + 2.0 * pad));
            (img, hot, 1.0)
        }
        _ => {
            let pts = [
                v2(0.0, 0.0),
                v2(0.0, 16.8),
                v2(4.1, 12.9),
                v2(6.9, 19.4),
                v2(9.3, 18.4),
                v2(6.6, 12.0),
                v2(12.2, 12.0),
            ];
            let units = v2(13.0, 20.0);
            let img = paint(units, PX, 1.3, |p| poly_sd(p, &pts));
            let hot = v2(pad / (units.x + 2.0 * pad), pad / (units.y + 2.0 * pad));
            (img, hot, (units.y + 2.0 * pad) / 20.0)
        }
    }
}

/// A soft circle for touch-style recordings.
pub fn touch_cursor() -> (Image, Vec2, f64) {
    let s = 96u32;
    let mut rgba = Vec::with_capacity((s * s * 4) as usize);
    for y in 0..s {
        for x in 0..s {
            let d = ((x as f64 + 0.5 - 48.0).hypot(y as f64 + 0.5 - 48.0)) - 30.0;
            let fill = (0.5 - d).clamp(0.0, 1.0) * 0.45;
            let ring = (1.0 - (d + 2.0).abs() / 2.0).clamp(0.0, 1.0) * 0.9;
            let a = (fill + ring).min(1.0);
            let c = (0.55 * fill + ring).min(a);
            rgba.extend_from_slice(&[(c * 255.0) as u8, (c * 255.0) as u8, (c * 255.0) as u8, (a * 255.0) as u8]);
        }
    }
    (Image { width: s, height: s, rgba }, v2(0.5, 0.5), 1.4)
}

//! Text rasterisation (Inter) for captions and keyboard badges.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};

use super::Image;

static SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/Inter-SemiBold.ttf");

pub fn key(text: &str, px: u32, badge: bool) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (text, px, badge).hash(&mut h);
    h.finish()
}

pub struct TextCache {
    font: FontRef<'static>,
}

impl Default for TextCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TextCache {
    pub fn new() -> Self {
        TextCache { font: FontRef::try_from_slice(SEMIBOLD).expect("Inter font") }
    }

    /// Measure a single line at `px`.
    pub fn width(&self, text: &str, px: f32) -> f32 {
        let f = self.font.as_scaled(PxScale::from(px));
        let mut w = 0.0;
        let mut prev = None;
        for ch in text.chars() {
            let id = f.glyph_id(ch);
            if let Some(p) = prev {
                w += f.kern(p, id);
            }
            w += f.h_advance(id);
            prev = Some(id);
        }
        w
    }

    /// Render white text on a dark rounded backdrop. Badges get a pill with
    /// a hairline border; captions a softer box.
    pub fn render(&self, text: &str, px: f32, badge: bool) -> Option<Image> {
        if text.is_empty() {
            return None;
        }
        let f = self.font.as_scaled(PxScale::from(px));
        let pad_x = if badge { px * 0.75 } else { px * 0.55 };
        let pad_y = if badge { px * 0.45 } else { px * 0.3 };
        let tw = self.width(text, px);
        let (w, h) = ((tw + 2.0 * pad_x).ceil() as u32 + 2, (f.ascent() - f.descent() + 2.0 * pad_y).ceil() as u32 + 2);
        let mut buf = vec![0f32; (w * h * 4) as usize];
        // Backdrop.
        let radius = if badge { h as f32 / 2.0 } else { px * 0.45 };
        let (bg, border) = if badge { ([0.08, 0.08, 0.09, 0.82], 0.18) } else { ([0.0, 0.0, 0.0, 0.62], 0.0) };
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let d = sd_round_rect(fx, fy, w as f32, h as f32, radius);
                let cov = (0.5 - d).clamp(0.0, 1.0);
                let rim = if border > 0.0 { (1.0 - (d + 1.0).abs()).clamp(0.0, 1.0) * border } else { 0.0 };
                let a = cov * bg[3];
                let i = ((y * w + x) * 4) as usize;
                let c = [bg[0] * a + rim, bg[1] * a + rim, bg[2] * a + rim];
                buf[i..i + 3].copy_from_slice(&c);
                buf[i + 3] = (a + rim).min(1.0);
            }
        }
        // Glyphs (white, premultiplied over the backdrop).
        let mut x = pad_x + 1.0;
        let baseline = pad_y + 1.0 + f.ascent();
        let mut prev = None;
        for ch in text.chars() {
            let id = f.glyph_id(ch);
            if let Some(p) = prev {
                x += f.kern(p, id);
            }
            let glyph = id.with_scale_and_position(PxScale::from(px), ab_glyph::point(x, baseline));
            if let Some(outlined) = self.font.outline_glyph(glyph) {
                let b = outlined.px_bounds();
                outlined.draw(|gx, gy, cov| {
                    let (px_, py_) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                    if px_ < 0 || py_ < 0 || px_ >= w as i32 || py_ >= h as i32 {
                        return;
                    }
                    let i = ((py_ as u32 * w + px_ as u32) * 4) as usize;
                    for c in 0..3 {
                        buf[i + c] = cov + buf[i + c] * (1.0 - cov);
                    }
                    buf[i + 3] = cov + buf[i + 3] * (1.0 - cov);
                });
            }
            x += f.h_advance(id);
            prev = Some(id);
        }
        let rgba = buf.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8).collect();
        Some(Image { width: w, height: h, rgba })
    }
}

fn sd_round_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let r = r.min(w / 2.0).min(h / 2.0);
    let qx = (x - w / 2.0).abs() - (w / 2.0 - r);
    let qy = (y - h / 2.0).abs() - (h / 2.0 - r);
    (qx.max(0.0).hypot(qy.max(0.0))) + qx.max(qy).min(0.0) - r
}

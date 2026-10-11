//! The Glide app icon, drawn procedurally with signed distance fields: a
//! rounded-square gradient tile, a floating app window with a highlighted
//! zoom region, and a cursor gliding in with motion streaks. Rendered at any
//! size so the .icns gets crisp pixels at every resolution.

type V = [f32; 2];

fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1]]
}
fn dot(a: V, b: V) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}
fn len(a: V) -> f32 {
    dot(a, a).sqrt()
}

fn rounded_box(p: V, c: V, half: V, r: f32) -> f32 {
    let q = [(p[0] - c[0]).abs() - half[0] + r, (p[1] - c[1]).abs() - half[1] + r];
    len([q[0].max(0.0), q[1].max(0.0)]) + q[0].max(q[1]).min(0.0) - r
}

fn circle(p: V, c: V, r: f32) -> f32 {
    len(sub(p, c)) - r
}

fn capsule(p: V, a: V, b: V, r: f32) -> f32 {
    let pa = sub(p, a);
    let ba = sub(b, a);
    let h = (dot(pa, ba) / dot(ba, ba)).clamp(0.0, 1.0);
    len([pa[0] - ba[0] * h, pa[1] - ba[1] * h]) - r
}

fn polygon(p: V, v: &[V]) -> f32 {
    let mut d = dot(sub(p, v[0]), sub(p, v[0]));
    let mut s = 1.0;
    let n = v.len();
    for i in 0..n {
        let j = (i + n - 1) % n;
        let e = sub(v[j], v[i]);
        let w = sub(p, v[i]);
        let h = (dot(w, e) / dot(e, e)).clamp(0.0, 1.0);
        let b = [w[0] - e[0] * h, w[1] - e[1] * h];
        d = d.min(dot(b, b));
        let c = [p[1] >= v[i][1], p[1] < v[j][1], e[0] * w[1] > e[1] * w[0]];
        if c.iter().all(|x| *x) || c.iter().all(|x| !*x) {
            s = -s;
        }
    }
    s * d.sqrt()
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn hex(c: u32) -> [f32; 3] {
    [((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0]
}

/// Premultiplied RGBA accumulator for one pixel.
struct Px([f32; 4]);

impl Px {
    fn over(&mut self, rgb: [f32; 3], a: f32) {
        let a = a.clamp(0.0, 1.0);
        for i in 0..3 {
            self.0[i] = rgb[i] * a + self.0[i] * (1.0 - a);
        }
        self.0[3] = a + self.0[3] * (1.0 - a);
    }
}

/// Cursor arrow outline, tip at the origin, ~1 unit tall.
const ARROW: [V; 7] = [[0.0, 0.0], [0.0, 0.74], [0.18, 0.58], [0.3, 0.86], [0.42, 0.81], [0.3, 0.54], [0.53, 0.54]];

/// Render the icon at `size`×`size` (RGBA8, straight alpha).
pub fn render(size: u32) -> Vec<u8> {
    let px = 1.0 / size as f32;
    let aa = |d: f32| (0.5 - d / px).clamp(0.0, 1.0);
    // Apple's icon grid: an 824/1024 tile centred on the canvas.
    let tile_c = [0.5, 0.5];
    let tile_h = [0.4023, 0.4023];
    let tile_r = 0.18;
    let card_c = [0.455, 0.45];
    let card_h = [0.255, 0.18];
    let card_r = 0.032;
    let bar = 0.05;
    let cursor_tip = [0.6, 0.555];
    let cursor_s = 0.215;
    let arrow: Vec<V> = ARROW.iter().map(|v| [cursor_tip[0] + v[0] * cursor_s, cursor_tip[1] + v[1] * cursor_s]).collect();

    let mut out = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let p = [(x as f32 + 0.5) * px, (y as f32 + 0.5) * px];
            let mut c = Px([0.0; 4]);

            // Tile shadow and tile.
            let d_tile = rounded_box(p, tile_c, tile_h, tile_r);
            let d_shadow = rounded_box(p, [tile_c[0], tile_c[1] + 0.012], tile_h, tile_r);
            c.over([0.0; 3], 0.28 * (1.0 - smoothstep(-0.01, 0.025, d_shadow)));
            let t = ((p[0] - 0.1) * 0.35 + (p[1] - 0.1) * 0.9).clamp(0.0, 1.0);
            let mut base = mix(hex(0x4f7cff), hex(0x5b2bd9), t);
            // Warm glow behind the cursor, cool sheen at the top.
            let glow = (1.0 - (len(sub(p, [0.78, 0.8])) / 0.45)).clamp(0.0, 1.0);
            base = mix(base, hex(0xff5fa8), glow * glow * 0.55);
            let sheen = (1.0 - (p[1] - 0.1) / 0.35).clamp(0.0, 1.0);
            base = mix(base, hex(0x9fd4ff), sheen * sheen * 0.22);
            c.over(base, aa(d_tile));
            // Thin inner highlight along the tile edge.
            c.over([1.0; 3], 0.12 * aa(d_tile.abs() - 0.0025) * (1.0 - p[1]));

            // Inside the tile only.
            let inside = aa(d_tile);
            if inside > 0.0 {
                // Card shadow.
                let d_cs = rounded_box(p, [card_c[0] + 0.006, card_c[1] + 0.028], card_h, card_r);
                c.over([0.05, 0.02, 0.2], inside * 0.38 * (1.0 - smoothstep(-0.03, 0.05, d_cs)));
                // Card body and title bar.
                let d_card = rounded_box(p, card_c, card_h, card_r);
                let body = aa(d_card) * inside;
                let top = card_c[1] - card_h[1];
                let in_bar = p[1] < top + bar;
                c.over(if in_bar { hex(0xe8e8ef) } else { hex(0xfbfbfd) }, body);
                if in_bar {
                    for (i, col) in [0xff5f57u32, 0xfebc2e, 0x28c840].iter().enumerate() {
                        let dc = circle(p, [card_c[0] - card_h[0] + 0.034 + i as f32 * 0.026, top + bar * 0.5], 0.0085);
                        c.over(hex(*col), aa(dc) * body);
                    }
                }
                // Content lines.
                let left = card_c[0] - card_h[0] + 0.035;
                for (i, w) in [0.2f32, 0.3, 0.24, 0.16].iter().enumerate() {
                    let yy = top + bar + 0.045 + i as f32 * 0.048;
                    let d_line = capsule(p, [left, yy], [left + w, yy], 0.011);
                    c.over(hex(0xd9dae6), aa(d_line) * body);
                }
                // Zoom focus region with a soft accent fill.
                let zc = [card_c[0] + 0.135, card_c[1] + 0.06];
                let zh = [0.085, 0.07];
                let d_zoom = rounded_box(p, zc, zh, 0.018);
                c.over(hex(0x4f7cff), 0.14 * aa(d_zoom) * body);
                c.over(hex(0x4f7cff), aa(d_zoom.abs() - 0.0055) * body);

                // Motion streaks trailing the cursor (down-right of it).
                let dir = [0.62f32, 0.78];
                for (i, (off, l, a)) in [(-0.035f32, 0.09f32, 0.75f32), (0.0, 0.13, 0.9), (0.035, 0.07, 0.6)].iter().enumerate() {
                    let n = [-dir[1], dir[0]];
                    let start = [cursor_tip[0] + 0.095 + n[0] * off + dir[0] * 0.05, cursor_tip[1] + 0.115 + n[1] * off + dir[1] * 0.05];
                    let end = [start[0] + dir[0] * l, start[1] + dir[1] * l];
                    let d_s = capsule(p, start, end, 0.011 - i as f32 * 0.001);
                    // Fade toward the tail.
                    let along = (dot(sub(p, start), dir) / l).clamp(0.0, 1.0);
                    c.over([1.0; 3], aa(d_s) * a * (1.0 - along * 0.85) * inside);
                }

                // Cursor shadow, white outline, black arrow.
                let d_arrow = polygon(p, &arrow);
                let shadow_arrow: Vec<V> = arrow.iter().map(|v| [v[0] + 0.006, v[1] + 0.014]).collect();
                let d_as = polygon(p, &shadow_arrow);
                c.over([0.05, 0.0, 0.15], inside * 0.35 * (1.0 - smoothstep(-0.012, 0.03, d_as)));
                c.over([1.0; 3], aa(d_arrow - 0.013) * inside);
                c.over(hex(0x111114), aa(d_arrow) * inside);
            }

            let i = ((y * size + x) * 4) as usize;
            let a = c.0[3];
            for k in 0..3 {
                let v = if a > 0.0 { c.0[k] / a } else { 0.0 };
                out[i + k] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
            out[i + 3] = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    out
}

/// Write a macOS `.iconset` folder (all sizes Apple's iconutil expects).
pub fn write_iconset(dir: &std::path::Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    for base in [16u32, 32, 128, 256, 512] {
        for scale in [1u32, 2] {
            let size = base * scale;
            let name = if scale == 1 { format!("icon_{base}x{base}.png") } else { format!("icon_{base}x{base}@2x.png") };
            image::save_buffer(dir.join(name), &render(size), size, size, image::ExtendedColorType::Rgba8)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_has_transparent_corners_and_opaque_centre() {
        let s = 64;
        let px = super::render(s);
        let at = |x: u32, y: u32| px[((y * s + x) * 4 + 3) as usize];
        assert_eq!(at(0, 0), 0);
        assert_eq!(at(s / 2, s / 2), 255);
    }
}

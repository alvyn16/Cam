//! The Glide app icon, drawn procedurally with signed distance fields: a
//! Big Sur-style squircle in a graphite-to-black gradient, a white cursor
//! gliding in on a tapered motion swoosh, and a small red record dot.
//! Rendered at any size so the .icns gets crisp pixels at every resolution.

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

fn circle(p: V, c: V, r: f32) -> f32 {
    len(sub(p, c)) - r
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

/// Distance to Apple's continuous-curvature tile: a superellipse
/// |x|^n + |y|^n = 1 (n = 5) of half-size `half`, centred on `c`. Exact on
/// the outline, first-order away from it (enough for anti-aliasing and soft
/// shadows).
fn squircle(p: V, c: V, half: f32) -> f32 {
    const N: f32 = 5.0;
    let (u, v) = (((p[0] - c[0]) / half).abs().max(1e-6), ((p[1] - c[1]) / half).abs().max(1e-6));
    let f = (u.powf(N) + v.powf(N)).powf(1.0 / N);
    let k = f.powf(1.0 - N) / half;
    let g = len([u.powf(N - 1.0) * k, v.powf(N - 1.0) * k]);
    (f - 1.0) / g
}

fn bezier(a: V, b: V, c: V, t: f32) -> V {
    let m = 1.0 - t;
    [m * m * a[0] + 2.0 * m * t * b[0] + t * t * c[0], m * m * a[1] + 2.0 * m * t * b[1] + t * t * c[1]]
}

/// Distance to a stroke along a quadratic curve whose radius grows from
/// `r0` at `a` to `r1` at `c`, and how far along it the nearest point is.
fn tapered(p: V, a: V, b: V, c: V, r0: f32, r1: f32) -> (f32, f32) {
    const STEPS: usize = 32;
    let mut best = (f32::MAX, 0.0);
    let mut prev = a;
    for i in 1..=STEPS {
        let t = i as f32 / STEPS as f32;
        let next = bezier(a, b, c, t);
        let pa = sub(p, prev);
        let ba = sub(next, prev);
        let h = (dot(pa, ba) / dot(ba, ba)).clamp(0.0, 1.0);
        let along = (i as f32 - 1.0 + h) / STEPS as f32;
        let d = len([pa[0] - ba[0] * h, pa[1] - ba[1] * h]) - (r0 + (r1 - r0) * along);
        if d < best.0 {
            best = (d, along);
        }
        prev = next;
    }
    best
}

/// Render the icon at `size`×`size` (RGBA8, straight alpha).
pub fn render(size: u32) -> Vec<u8> {
    let px = 1.0 / size as f32;
    let aa = |d: f32| (0.5 - d / px).clamp(0.0, 1.0);
    // Apple's icon grid: an 824/1024 tile centred on the canvas.
    let tile_c = [0.5, 0.5];
    let tile_h = 0.4023;
    // The mark: a cursor gliding up and to the left on a curved trail.
    let tip = [0.33, 0.285];
    let scale = 0.4;
    let arrow: Vec<V> = ARROW.iter().map(|v| [tip[0] + v[0] * scale, tip[1] + v[1] * scale]).collect();
    // Trail: a gentle dip from the lower right, thin, rising into the
    // cursor's tail, full.
    let (t0, t1, t2) = ([0.85, 0.7], [0.7, 0.8], [0.49, 0.62]);
    let dot_c = [0.705, 0.31];
    let dot_r = 0.05;

    let mut out = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let p = [(x as f32 + 0.5) * px, (y as f32 + 0.5) * px];
            let mut c = Px([0.0; 4]);

            // Soft drop shadow under the tile, then the tile.
            let d_shadow = squircle(p, [tile_c[0], tile_c[1] + 0.01], tile_h);
            c.over([0.0; 3], 0.32 * (1.0 - smoothstep(-0.012, 0.03, d_shadow)));
            let d_tile = squircle(p, tile_c, tile_h);
            let inside = aa(d_tile);
            // Graphite at the top to near-black at the bottom, with a faint
            // cool sheen in the upper half.
            let t = ((p[1] - 0.1) / 0.8).clamp(0.0, 1.0);
            let mut base = mix(hex(0x3c3f47), hex(0x0d0e11), t.powf(0.85));
            let sheen = (1.0 - len(sub(p, [0.42, 0.12])) / 0.55).clamp(0.0, 1.0);
            base = mix(base, hex(0x6b7280), sheen * sheen * 0.35);
            c.over(base, inside);
            // A hairline along the edge: light at the top, fading out below.
            let rim = aa(d_tile.abs() - 0.0022) * inside;
            c.over([1.0; 3], rim * 0.22 * (1.0 - (p[1] - 0.1) / 0.5).clamp(0.0, 1.0));

            if inside > 0.0 {
                // Motion trail: a tapered white swoosh, fading toward its tail.
                let (d_trail, along) = tapered(p, t0, t1, t2, 0.004, 0.034);
                c.over([1.0; 3], aa(d_trail) * inside * (0.1 + 0.62 * along * along));
                // A shorter, fainter streak running alongside, below it.
                let (d_streak, along2) = tapered(p, [0.8, 0.79], [0.67, 0.86], [0.52, 0.74], 0.003, 0.016);
                c.over([1.0; 3], aa(d_streak) * inside * (0.06 + 0.4 * along2 * along2));

                // Cursor: soft shadow, then a crisp white arrow with rounded
                // corners and a faint top-to-bottom shade.
                let shadow_arrow: Vec<V> = arrow.iter().map(|v| [v[0] + 0.008, v[1] + 0.018]).collect();
                let d_as = polygon(p, &shadow_arrow) - 0.01;
                c.over([0.0; 3], inside * 0.5 * (1.0 - smoothstep(-0.015, 0.035, d_as)));
                let d_arrow = polygon(p, &arrow) - 0.01;
                let shade = ((p[1] - tip[1]) / (0.86 * scale)).clamp(0.0, 1.0);
                c.over(mix([1.0; 3], hex(0xe4e6eb), shade), aa(d_arrow) * inside);

                // Record dot: a soft red glow, then the dot, lit from above.
                let d_dot = circle(p, dot_c, dot_r);
                c.over(hex(0xff3b30), inside * 0.28 * (1.0 - smoothstep(-0.01, 0.045, d_dot)));
                let lit = ((p[1] - (dot_c[1] - dot_r)) / (2.0 * dot_r)).clamp(0.0, 1.0);
                c.over(mix(hex(0xff6a5f), hex(0xe5241b), lit), aa(d_dot) * inside);
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

    /// Graphite, white and a red dot: nothing purple (or blue-violet).
    #[test]
    fn icon_has_no_purple() {
        let px = super::render(256);
        let purple = px
            .chunks_exact(4)
            .filter(|p| p[3] > 32)
            .filter(|p| {
                let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
                b > 70 && b - g > 30 && b - r > -40 && r - g > 10
            })
            .count();
        assert_eq!(purple, 0, "{purple} purple pixels");
    }
}

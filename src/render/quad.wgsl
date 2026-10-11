// One shader for every layer. Each draw is a (possibly rotated) quad in
// output pixels with a mode that decides how its pixels are coloured.
// Colours are premultiplied alpha; blending is One / OneMinusSrcAlpha.

const MODE_TEXTURE: u32 = 0u;
const MODE_SHADOW: u32 = 1u;
const MODE_WINDOW: u32 = 2u;
const MODE_CIRCLE: u32 = 3u;
const MODE_GRADIENT: u32 = 4u;
const MODE_COMPOSITE: u32 = 5u;
const MODE_MASK: u32 = 6u;
const MODE_DIM: u32 = 7u;
const MODE_FILL: u32 = 8u;

const PI: f32 = 3.14159265;

struct Draw {
    // Quad rect in output px (x, y, w, h), before rotation and expansion.
    rect: vec4<f32>,
    // Rotation pivot (px), rotation (radians), expansion margin (px).
    xform: vec4<f32>,
    // Texture sub-rect (x, y, w, h), normalised.
    uv: vec4<f32>,
    // Tint/fill colour (straight alpha).
    color: vec4<f32>,
    // Shape rect for SDF masks (x, y, w, h) in output px.
    shape: vec4<f32>,
    // radius, inner radius, inset, opacity
    p0: vec4<f32>,
    // inset colour (straight alpha)
    p1: vec4<f32>,
    // blur radius px, velocity x/y px, flags (1 = mirror x, 2 = rounded mask)
    p2: vec4<f32>,
    // ripple centre x/y, progress, strength
    p3: vec4<f32>,
    // ripple reach px, zoom-blur strength, zoom centre x/y
    p4: vec4<f32>,
    // output size w, h, mode, stop count
    out: vec4<f32>,
    // gradient: direction (x, y), unused
    g0: vec4<f32>,
    // gradient stop colours and positions
    stops: array<vec4<f32>, 4>,
    stop_pos: vec4<f32>,
};

@group(0) @binding(0) var<uniform> d: Draw;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    // Output-pixel position of this fragment (unrotated quad space).
    @location(0) local: vec2<f32>,
};

fn rot(v: vec2<f32>, a: f32) -> vec2<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec2<f32>(c * v.x - s * v.y, s * v.x + c * v.y);
}

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let c = corners[vi];
    let m = d.xform.w;
    let lo = d.rect.xy - vec2<f32>(m, m);
    let size = d.rect.zw + vec2<f32>(2.0 * m, 2.0 * m);
    let local = lo + c * size;
    let pivot = d.xform.xy;
    let world = pivot + rot(local - pivot, d.xform.z);
    let ndc = vec2<f32>(world.x / d.out.x * 2.0 - 1.0, 1.0 - world.y / d.out.y * 2.0);
    var o: VsOut;
    o.pos = vec4<f32>(ndc, 0.0, 1.0);
    o.local = local;
    return o;
}

fn sd_round_rect(p: vec2<f32>, center: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let rr = min(r, min(half.x, half.y));
    let q = abs(p - center) - half + vec2<f32>(rr, rr);
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - rr;
}

fn coverage(dist: f32) -> f32 {
    return clamp(0.5 - dist, 0.0, 1.0);
}

// Poisson disc offsets for soft blur.
const DISC = array<vec2<f32>, 12>(
    vec2<f32>(-0.326, -0.406), vec2<f32>(-0.840, -0.074), vec2<f32>(-0.696, 0.457),
    vec2<f32>(-0.203, 0.621), vec2<f32>(0.962, -0.195), vec2<f32>(0.473, -0.480),
    vec2<f32>(0.519, 0.767), vec2<f32>(0.185, -0.893), vec2<f32>(0.507, 0.064),
    vec2<f32>(0.896, 0.412), vec2<f32>(-0.322, -0.933), vec2<f32>(-0.792, -0.598),
);

fn sample_at(uv: vec2<f32>, lod: f32) -> vec4<f32> {
    return textureSampleLevel(tex, samp, uv, lod);
}

// Sample with an optional blur radius given in texture pixels.
fn sample_blur(uv: vec2<f32>, radius_px: f32) -> vec4<f32> {
    if radius_px < 0.5 {
        return sample_at(uv, 0.0);
    }
    let dims = vec2<f32>(textureDimensions(tex, 0));
    let lod = max(log2(radius_px / 3.0), 0.0);
    var acc = sample_at(uv, lod) * 2.0;
    for (var i = 0; i < 12; i++) {
        acc += sample_at(uv + DISC[i] * radius_px / dims, lod);
    }
    return acc / 14.0;
}

// Texture uv for an output-pixel position inside the quad rect.
fn rect_uv(local: vec2<f32>) -> vec2<f32> {
    var t = (local - d.rect.xy) / d.rect.zw;
    if (u32(d.p2.w) & 1u) != 0u {
        t.x = 1.0 - t.x;
    }
    return d.uv.xy + t * d.uv.zw;
}

fn premul(c: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(c.rgb * c.a, c.a);
}

fn erf2(x: vec2<f32>) -> vec2<f32> {
    let s = sign(x);
    let a = abs(x);
    var r = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    r = r * r;
    return s - s / (r * r);
}

fn gaussian(x: f32, sigma: f32) -> f32 {
    return exp(-(x * x) / (2.0 * sigma * sigma)) / (sqrt(2.0 * PI) * sigma);
}

fn shadow_x(x: f32, y: f32, sigma: f32, corner: f32, half: vec2<f32>) -> f32 {
    let delta = min(half.y - corner - abs(y), 0.0);
    let curved = half.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
    let integral = 0.5 + 0.5 * erf2((vec2<f32>(x, x) + vec2<f32>(-curved, curved)) * (sqrt(0.5) / sigma));
    return integral.y - integral.x;
}

// Gaussian-blurred rounded rectangle (analytic in x, sampled in y).
fn rounded_box_shadow(lo: vec2<f32>, hi: vec2<f32>, p_in: vec2<f32>, sigma: f32, corner: f32) -> f32 {
    let center = (lo + hi) * 0.5;
    let half = (hi - lo) * 0.5;
    let p = p_in - center;
    let low = p.y - half.y;
    let high = p.y + half.y;
    let start = clamp(-3.0 * sigma, low, high);
    let end = clamp(3.0 * sigma, low, high);
    let step = (end - start) / 4.0;
    var y = start + step * 0.5;
    var v = 0.0;
    for (var i = 0; i < 4; i++) {
        v += shadow_x(p.x, p.y - y, sigma, corner, half) * gaussian(y, sigma) * step;
        y += step;
    }
    return v;
}

fn mode_texture(local: vec2<f32>) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(tex, 0));
    let uv = rect_uv(local);
    // Blur radius is given in output px; convert to texture px.
    let px_per_out = dims.x * d.uv.z / max(d.rect.z, 1.0);
    let blur = d.p2.x * px_per_out;
    var c = vec4<f32>(0.0);
    let vel = d.p2.yz;
    let vlen = length(vel);
    if vlen > 0.5 {
        // Motion smear along the velocity, centred on the current position.
        let n = 9;
        for (var i = 0; i < n; i++) {
            let k = f32(i) / f32(n - 1) - 0.5;
            let lp = local - rot(vel * k, -d.xform.z);
            let u = rect_uv(lp);
            let inside = all(lp >= d.rect.xy) && all(lp <= d.rect.xy + d.rect.zw);
            if inside {
                c += sample_blur(u, blur);
            }
        }
        c = c / f32(n);
    } else {
        let inside = all(local >= d.rect.xy) && all(local <= d.rect.xy + d.rect.zw);
        if inside {
            c = sample_blur(uv, blur);
        }
    }
    // Texture is premultiplied; apply tint and opacity.
    c = vec4<f32>(c.rgb * d.color.rgb, c.a) * d.color.a * d.p0.w;
    if (u32(d.p2.w) & 2u) != 0u {
        let half = d.shape.zw * 0.5;
        let dist = sd_round_rect(local, d.shape.xy + half, half, d.p0.x);
        c = c * coverage(dist);
    }
    return c;
}

fn mode_shadow(local: vec2<f32>) -> vec4<f32> {
    let sigma = max(d.p2.x, 0.5);
    let v = rounded_box_shadow(d.shape.xy, d.shape.xy + d.shape.zw, local, sigma, d.p0.x);
    let a = clamp(v, 0.0, 1.0) * d.color.a * d.p0.w;
    return vec4<f32>(d.color.rgb * a, a);
}

fn ripple_offset(p: vec2<f32>) -> vec2<f32> {
    let strength = d.p3.w;
    if strength <= 0.0 {
        return vec2<f32>(0.0);
    }
    let c = d.p3.xy;
    let progress = d.p3.z;
    let reach = d.p4.x;
    let to = p - c;
    let dist = length(to);
    let ring = progress * reach;
    let band = reach * 0.18;
    let x = (dist - ring) / band;
    if abs(x) > 1.0 || dist < 0.001 {
        return vec2<f32>(0.0);
    }
    let wave = sin(x * PI) * (1.0 - abs(x));
    let amp = strength * reach * 0.012 * (1.0 - progress);
    return (to / dist) * wave * amp;
}

fn mode_window(local: vec2<f32>) -> vec4<f32> {
    let outer_half = d.shape.zw * 0.5;
    let center = d.shape.xy + outer_half;
    let outer = sd_round_rect(local, center, outer_half, d.p0.x);
    let cov = coverage(outer);
    if cov <= 0.0 {
        return vec4<f32>(0.0);
    }
    let inset = d.p0.z;
    let inner_half = outer_half - vec2<f32>(inset, inset);
    let inner = sd_round_rect(local, center, inner_half, d.p0.y);
    let inner_cov = coverage(inner);
    var screen = vec4<f32>(0.0);
    if inner_cov > 0.0 {
        let p = local - ripple_offset(local);
        let lo = center - inner_half;
        let t = clamp((p - lo) / (inner_half * 2.0), vec2<f32>(0.0), vec2<f32>(1.0));
        screen = sample_at(d.uv.xy + t * d.uv.zw, 0.0);
        screen = vec4<f32>(screen.rgb, 1.0);
    }
    let inset_col = premul(d.p1);
    let col = screen * inner_cov + inset_col * (1.0 - inner_cov);
    return col * cov * d.p0.w;
}

fn mode_circle(local: vec2<f32>) -> vec4<f32> {
    let half = d.shape.zw * 0.5;
    let c = d.shape.xy + half;
    let r = half.x;
    let dist = length(local - c) - r;
    let soft = max(d.p2.x, 0.75);
    let a = clamp(0.5 - dist / soft, 0.0, 1.0) * d.color.a * d.p0.w;
    // Thin darker rim.
    let rim = clamp(1.0 - abs(dist + 1.0) / 1.5, 0.0, 1.0) * 0.5 * d.p0.w;
    let fill = d.color.rgb * a;
    return vec4<f32>(fill * (1.0 - rim), max(a, rim * a));
}

fn mode_gradient(local: vec2<f32>) -> vec4<f32> {
    let uv = (local - d.rect.xy) / d.rect.zw;
    let dir = d.g0.xy;
    // Project onto the direction across the unit square.
    let t = clamp(dot(uv - vec2<f32>(0.5, 0.5), dir) / (abs(dir.x) * 0.5 + abs(dir.y) * 0.5) * 0.5 + 0.5, 0.0, 1.0);
    let n = i32(d.out.w);
    var c = d.stops[0];
    for (var i = 1; i < 4; i++) {
        if i < n {
            let a = d.stop_pos[i - 1];
            let b = d.stop_pos[i];
            if t >= a {
                c = mix(d.stops[i - 1], d.stops[i], clamp((t - a) / max(b - a, 0.0001), 0.0, 1.0));
            }
        }
    }
    return premul(vec4<f32>(c.rgb, c.a * d.p0.w));
}

fn mode_composite(local: vec2<f32>) -> vec4<f32> {
    let size = d.out.xy;
    let uv = local / size;
    let vel = d.p2.yz;
    let zs = d.p4.y;
    let n = 15;
    if length(vel) > 0.5 {
        var c = vec4<f32>(0.0);
        for (var i = 0; i < n; i++) {
            let k = f32(i) / f32(n - 1) - 0.5;
            c += sample_at(uv - vel * k / size, 0.0);
        }
        return c / f32(n) * d.p0.w;
    }
    if zs > 0.0005 {
        let zc = d.p4.zw;
        var c = vec4<f32>(0.0);
        for (var i = 0; i < n; i++) {
            let k = f32(i) / f32(n - 1) - 0.5;
            let p = zc + (local - zc) * (1.0 + zs * k);
            c += sample_at(p / size, 0.0);
        }
        return c / f32(n) * d.p0.w;
    }
    return sample_at(uv, 0.0) * d.p0.w;
}

// Average of a block of the screen texture (4x4 taps), clamped to the
// visible crop so cropped-out pixels never leak in.
fn block_avg(center: vec2<f32>, block: vec2<f32>) -> vec4<f32> {
    let lo = d.p3.xy;
    let hi = d.p3.xy + d.p3.zw;
    var acc = vec4<f32>(0.0);
    for (var y = 0; y < 4; y++) {
        for (var x = 0; x < 4; x++) {
            let o = (vec2<f32>(f32(x), f32(y)) + 0.5) / 4.0 - 0.5;
            acc += sample_at(clamp(center + o * block, lo, hi), 0.0);
        }
    }
    return acc / 16.0;
}

// Blur or pixelate the screen inside a rounded rect. The blur is a smooth
// mosaic: block averages on a grid fixed in texture space, blended with a
// smoothstep, which is stable while the window moves and zooms.
fn mode_mask(local: vec2<f32>) -> vec4<f32> {
    let half = d.shape.zw * 0.5;
    let cov = coverage(sd_round_rect(local, d.shape.xy + half, half, d.p0.x));
    if cov <= 0.0 {
        return vec4<f32>(0.0);
    }
    let dims = vec2<f32>(textureDimensions(tex, 0));
    let px_per_out = dims.x * d.uv.z / max(d.rect.z, 1.0);
    let block = vec2<f32>(max(d.p2.x * px_per_out, 2.0)) / dims;
    let uv = rect_uv(local);
    var c: vec4<f32>;
    if (u32(d.p2.w) & 4u) != 0u {
        c = block_avg((floor(uv / block) + 0.5) * block, block);
    } else {
        let g = uv / block - 0.5;
        let i = floor(g);
        let f = fract(g);
        let s = f * f * (3.0 - 2.0 * f);
        let c00 = block_avg((i + vec2<f32>(0.5, 0.5)) * block, block);
        let c10 = block_avg((i + vec2<f32>(1.5, 0.5)) * block, block);
        let c01 = block_avg((i + vec2<f32>(0.5, 1.5)) * block, block);
        let c11 = block_avg((i + vec2<f32>(1.5, 1.5)) * block, block);
        c = mix(mix(c00, c10, s.x), mix(c01, c11, s.x), s.y);
    }
    return vec4<f32>(c.rgb, 1.0) * cov * d.p0.w;
}

// Dim the screen outside a rounded rect (spotlight). p3 = screen rect,
// p0 = (hole radius, screen radius, feather px, opacity).
fn mode_dim(local: vec2<f32>) -> vec4<f32> {
    let sh = d.p3.zw * 0.5;
    let screen = coverage(sd_round_rect(local, d.p3.xy + sh, sh, d.p0.y));
    let half = d.shape.zw * 0.5;
    let feather = max(d.p0.z, 1.0);
    let hole = clamp(0.5 - sd_round_rect(local, d.shape.xy + half, half, d.p0.x) / feather, 0.0, 1.0);
    let a = d.color.a * d.p0.w * screen * (1.0 - hole);
    return vec4<f32>(d.color.rgb * a, a);
}

// Rounded rect with a vertical gradient (color → p1) and an inner border
// (p4 colour, p2.x width). Used for device frames.
fn mode_fill(local: vec2<f32>) -> vec4<f32> {
    let half = d.shape.zw * 0.5;
    let dist = sd_round_rect(local, d.shape.xy + half, half, d.p0.x);
    let cov = coverage(dist);
    if cov <= 0.0 {
        return vec4<f32>(0.0);
    }
    let t = clamp((local.y - d.shape.y) / max(d.shape.w, 1.0), 0.0, 1.0);
    var c = mix(d.color, d.p1, t);
    let bw = d.p2.x;
    if bw > 0.0 {
        // 1 at the outer edge fading to 0 at `bw` inside.
        let edge = clamp(1.0 - (-dist) / bw, 0.0, 1.0);
        c = mix(c, d.p4, edge);
    }
    return premul(vec4<f32>(c.rgb, c.a * cov * d.p0.w));
}

@fragment
fn fs(i: VsOut) -> @location(0) vec4<f32> {
    let mode = u32(d.out.z);
    switch mode {
        case 1u: { return mode_shadow(i.local); }
        case 2u: { return mode_window(i.local); }
        case 3u: { return mode_circle(i.local); }
        case 4u: { return mode_gradient(i.local); }
        case 5u: { return mode_composite(i.local); }
        case 6u: { return mode_mask(i.local); }
        case 7u: { return mode_dim(i.local); }
        case 8u: { return mode_fill(i.local); }
        default: { return mode_texture(i.local); }
    }
}

// The city around the tower, ray cast per pixel from a camera that turns and tilts with the
// office, so it holds together from any angle: sky and the painted skyline on a cylinder at the
// horizon, the painted rooftops below it, and looking down, a street grid of rooftop lots with
// traffic, lined up with the office walls. World axes are the room's: x and y along the floor
// (tiles), z up; the street is z = 0 and the camera floats above it.

struct Sky {
    right: vec4<f32>,
    up: vec4<f32>,
    fwd: vec4<f32>,
    // xyz: camera position; w: seconds
    cam: vec4<f32>,
    // tan of the half field of view (x, y), night 0/1, sRGB target 0/1
    lens: vec4<f32>,
};

@group(0) @binding(0) var<uniform> s: Sky;
@group(1) @binding(0) var far_tex: texture_2d<f32>;
@group(1) @binding(1) var near_tex: texture_2d<f32>;
@group(1) @binding(2) var ground_tex: texture_2d<f32>;
@group(1) @binding(3) var samp: sampler;

const TAU: f32 = 6.2831853;
// the skyline painting: twice around the horizon, from just under it to ~22° up (tan units)
const FAR_REPEAT: f32 = 2.0;
const FAR_TOP: f32 = 0.40;
const FAR_BOT: f32 = -0.015;
// the rooftop painting, from a little under the horizon down to ~32° (tan of the depression);
// between it and the skyline lies the haze over the far city
const NEAR_REPEAT: f32 = 2.0;
const NEAR_TOP: f32 = 0.10;
const NEAR_ROWS: f32 = 212.0;
const NEAR_BOT: f32 = 0.62;
// the street grid: two square lots per block along x, avenues between blocks, streets along x
const LOT: f32 = 110.0;
const AVENUE: f32 = 18.0;
const STREET: f32 = 12.0;
const WALK: f32 = 2.6;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VOut {
    // one triangle over the whole stage
    let p = vec2(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    var o: VOut;
    o.pos = vec4(p, 0.0, 1.0);
    o.ndc = p;
    return o;
}

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), u.x), mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), u.x), u.y);
}

fn fbm(p: vec2<f32>) -> f32 {
    var v = 0.0;
    var a = 0.5;
    var q = p;
    for (var k = 0; k < 4; k++) {
        v += a * noise(q);
        q = q * 2.03 + vec2(17.0, 9.0);
        a *= 0.5;
    }
    return v;
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3(2.4));
    return select(hi, lo, c <= vec3(0.04045));
}

// coverage of a box of half size `b` around the origin, antialiased over `fw` (world per px)
fn box(p: vec2<f32>, b: vec2<f32>, fw: f32) -> f32 {
    let d = abs(p) - b;
    let dist = length(max(d, vec2(0.0))) + min(max(d.x, d.y), 0.0);
    return clamp(0.5 - dist / fw, 0.0, 1.0);
}

// one lot's rooftops, turned and mirrored by its own hash so neighbours differ
fn orient(v: vec2<f32>, k: u32) -> vec2<f32> {
    var r = v;
    if ((k & 4u) != 0u) {
        r = vec2(-r.x, r.y);
    }
    switch (k & 3u) {
        case 1u: { r = vec2(-r.y, r.x); }
        case 2u: { r = -r; }
        case 3u: { r = vec2(r.y, -r.x); }
        default: {}
    }
    return r;
}

struct Lane {
    // distance across the road from the lane's centre, and along it (global)
    across: f32,
    along: f32,
    dir: f32,
    seed: f32,
};

// a car in a lane, if one is here: rgb premultiplied by coverage, alpha = coverage
fn car(l: Lane, t: f32, fw: f32, night: f32) -> vec4<f32> {
    let speed = 7.0 + 4.0 * hash(vec2(l.seed, 3.1));
    let gap = 15.0;
    let s = l.along * l.dir - t * speed;
    let id = floor(s / gap);
    let h = hash(vec2(id, l.seed));
    if (h > 0.55) {
        return vec4(0.0);
    }
    let len = 4.4;
    let local = s - id * gap - h * 6.0;
    let p = vec2(l.across, local - len / 2.0);
    let body = box(p, vec2(0.95, len / 2.0), fw);
    // front is +local
    let pick = hash(vec2(id, l.seed + 7.0));
    var col = vec3(0.85, 0.86, 0.88);
    if (pick < 0.35) {
        col = vec3(0.97, 0.78, 0.22);
    } else if (pick < 0.5) {
        col = vec3(0.20, 0.22, 0.27);
    } else if (pick < 0.62) {
        col = vec3(0.70, 0.18, 0.17);
    } else if (pick < 0.75) {
        col = vec3(0.26, 0.38, 0.62);
    }
    let roof = box(p - vec2(0.0, -0.25), vec2(0.7, 1.15), fw);
    let glass = box(p - vec2(0.0, 0.95), vec2(0.72, 0.32), fw) + box(p - vec2(0.0, -1.45), vec2(0.7, 0.22), fw);
    var c = col * (1.0 - 0.12 * roof);
    c = mix(c, vec3(0.16, 0.2, 0.26), clamp(glass, 0.0, 1.0));
    c = mix(c, c * 0.35, night);
    var out = vec4(c * body, body);
    if (night > 0.5) {
        // headlights ahead, tail lights behind
        let beam = exp(-pow(l.across / 1.1, 2.0)) * smoothstep(9.0, 2.2, local - len) * step(len, local) * 0.55;
        let head = box(vec2(abs(l.across) - 0.6, local - len + 0.15), vec2(0.22, 0.18), fw);
        let tail = box(vec2(abs(l.across) - 0.6, local - 0.12), vec2(0.22, 0.14), fw);
        let glow = vec3(1.0, 0.94, 0.78) * (beam + head) + vec3(1.0, 0.18, 0.12) * tail;
        out = vec4(out.rgb + glow, max(out.a, clamp(beam + head + tail, 0.0, 1.0)));
    }
    return out;
}

// the street level at ground point p
fn ground(p: vec2<f32>, gx: vec2<f32>, gy: vec2<f32>, t: f32, night: f32) -> vec3<f32> {
    let fw = max(max(length(gx), length(gy)), 0.02);
    let period = vec2(2.0 * LOT + AVENUE, LOT + STREET);
    let cell = floor(p / period);
    let l = p - cell * period;
    let on_avenue = l.x >= 2.0 * LOT;
    let on_street = l.y >= LOT;

    // rooftops
    let lot = min(floor(l.x / LOT), 1.0);
    let k = u32(hash(cell * vec2(2.0, 1.0) + vec2(lot, 0.0) + 17.0) * 8.0);
    let lp = vec2(l.x - lot * LOT, l.y) / LOT;
    let uv = orient(lp - 0.5, k) + 0.5;
    let roofs = textureSampleGrad(ground_tex, samp, uv, orient(gx / LOT, k), orient(gy / LOT, k)).rgb;

    let asphalt = mix(vec3(0.38, 0.39, 0.42), vec3(0.11, 0.12, 0.16), night);
    let walk = mix(vec3(0.76, 0.75, 0.72), vec3(0.21, 0.23, 0.29), night);
    let paint = mix(vec3(0.92, 0.92, 0.88), vec3(0.45, 0.46, 0.48), night);
    let a = l.x - 2.0 * LOT;
    let b = l.y - LOT;
    var c = roofs;
    if (on_avenue || on_street) {
        c = asphalt;
        // sidewalks along the lots, with a curb line
        let walk_av = on_avenue && !on_street && (a < WALK || a > AVENUE - WALK);
        let walk_st = on_street && !on_avenue && (b < WALK || b > STREET - WALK);
        if (walk_av || walk_st) {
            c = walk;
        }
        let curb_av = on_avenue && !on_street && min(abs(a - WALK), abs(a - AVENUE + WALK)) < fw * 0.8;
        let curb_st = on_street && !on_avenue && min(abs(b - WALK), abs(b - STREET + WALK)) < fw * 0.8;
        if (curb_av || curb_st) {
            c = c * 0.7;
        }
        // lane dashes on the avenue, a centre line on the street
        if (on_avenue && !on_street) {
            let lanes = (AVENUE - 2.0 * WALK) / 3.0;
            let m = min(abs(a - WALK - lanes), abs(a - WALK - 2.0 * lanes));
            let dash = step(fract(p.y / 7.0), 0.5);
            c = mix(c, paint, clamp(0.5 - (m - 0.14) / fw, 0.0, 1.0) * dash);
        }
        if (on_street && !on_avenue) {
            let m = abs(b - STREET / 2.0);
            let dash = step(fract(p.x / 7.0), 0.55);
            c = mix(c, mix(vec3(0.92, 0.76, 0.28), vec3(0.42, 0.36, 0.2), night), clamp(0.5 - (m - 0.12) / fw, 0.0, 1.0) * dash);
        }
        // zebra crossings where the street meets the avenue
        if (on_avenue && on_street) {
            let zx = step(fract(a / 1.3), 0.5) * (step(b, 2.2) + step(STREET - 2.2, b));
            let zy = step(fract(b / 1.3), 0.5) * (step(a, 2.2) + step(AVENUE - 2.2, a));
            c = mix(c, paint, clamp(zx + zy, 0.0, 1.0) * 0.85);
        }
        // traffic: one way on each avenue, both ways on the streets, none parked in the crossings
        let av_dir = select(-1.0, 1.0, hash(vec2(cell.x, 5.0)) < 0.5);
        if (on_avenue) {
            let lanes = (AVENUE - 2.0 * WALK) / 3.0;
            for (var i = 0; i < 3; i++) {
                let centre = WALK + lanes * (f32(i) + 0.5);
                let v = car(Lane(a - centre, p.y, av_dir, cell.x * 3.0 + f32(i)), t, fw, night);
                c = c * (1.0 - v.a) + v.rgb;
            }
        }
        if (on_street && !on_avenue) {
            for (var i = 0; i < 2; i++) {
                let centre = WALK + (STREET - 2.0 * WALK) * (f32(i) * 0.5 + 0.25);
                let v = car(Lane(b - centre, p.x, select(1.0, -1.0, i == 0), cell.y * 2.0 + f32(i) + 50.0), t, fw, night);
                c = c * (1.0 - v.a) + v.rgb;
            }
        }
    }
    if (night > 0.5) {
        // street lamps along the curbs, warm pools on the road
        let lamp_av = vec2(select(AVENUE - WALK + 0.6, WALK - 0.6, fract(p.y / 26.0) < 0.5), (floor(p.y / 13.0) + 0.5) * 13.0);
        let d_av = vec2(a - lamp_av.x, p.y - lamp_av.y);
        let pool_av = select(0.0, exp(-dot(d_av, d_av) / 14.0), on_avenue);
        let lamp_st = vec2((floor(p.x / 13.0) + 0.5) * 13.0, select(STREET - WALK + 0.6, WALK - 0.6, fract(p.x / 26.0) < 0.5));
        let d_st = vec2(p.x - lamp_st.x, b - lamp_st.y);
        let pool_st = select(0.0, exp(-dot(d_st, d_st) / 12.0), on_street && !on_avenue);
        c += vec3(1.0, 0.78, 0.45) * (pool_av + pool_st) * 0.42;
    }
    return c;
}

// the bird nearest this direction, as coverage
fn birds(d: vec3<f32>, ndc: vec2<f32>, t: f32) -> f32 {
    var cov = 0.0;
    let ppx = 2.0 / 700.0;
    for (var i = 0; i < 7; i++) {
        let fi = f32(i);
        let az = fract(0.12 + fi * 0.013 + t / (46.0 + fi * 1.9)) * TAU;
        let el = 0.13 + 0.025 * fract(fi * 0.37) + 0.012 * sin(t * 0.4 + fi);
        let bd = normalize(vec3(sin(az), cos(az), el));
        let depth = dot(bd, s.fwd.xyz);
        if (depth <= 0.05) {
            continue;
        }
        let bp = vec2(dot(bd, s.right.xyz) / depth / s.lens.x, dot(bd, s.up.xyz) / depth / s.lens.y);
        // in screen px-ish units, aspect corrected
        var q = (ndc - bp) / ppx;
        q.x *= s.lens.x / s.lens.y;
        let flap = select(-1.0, 1.0, fract(t * 2.2 + fi * 0.37) < 0.5);
        // an M or a flattened W: two short strokes
        let x = abs(q.x);
        let y = q.y - flap * (x * 0.55 - 2.0);
        let stroke = smoothstep(1.6, 0.6, abs(y)) * smoothstep(9.0, 7.0, x);
        cov = max(cov, stroke);
    }
    return cov;
}

@fragment
fn fs(o: VOut) -> @location(0) vec4<f32> {
    let night = s.lens.z;
    let t = s.cam.w;
    let d = normalize(s.fwd.xyz + o.ndc.x * s.lens.x * s.right.xyz + o.ndc.y * s.lens.y * s.up.xyz);
    let flat_len = max(length(d.xy), 1e-4);
    let tz = d.z / flat_len;
    let az = atan2(d.x, d.y) / TAU;
    // ground hit (only used below the horizon)
    let dist = s.cam.z / max(-d.z, 1e-3);
    let gp = s.cam.xy + d.xy * dist;

    // derivatives up front, in uniform control flow; the azimuth wraps once around
    var az_dx = dpdx(az);
    var az_dy = dpdy(az);
    az_dx -= round(az_dx);
    az_dy -= round(az_dy);
    let tz_dx = dpdx(tz);
    let tz_dy = dpdy(tz);
    let gp_dx = dpdx(gp);
    let gp_dy = dpdy(gp);

    // sky
    let zenith = mix(vec3(0.52, 0.76, 0.93), vec3(0.035, 0.05, 0.13), night);
    let horizon = mix(vec3(0.80, 0.91, 0.95), vec3(0.11, 0.15, 0.25), night);
    var col = mix(horizon, zenith, smoothstep(0.0, 1.2, tz));
    if (tz > 0.0) {
        // drifting clouds on a high plane; a few dim ones at night
        let cp = d.xy / max(d.z, 0.02) * 0.0016 + vec2(t * 0.004, t * 0.0015);
        let n = fbm(cp * 3.0);
        let cover = smoothstep(0.55, 0.82, n) * smoothstep(0.03, 0.25, tz);
        col = mix(col, mix(vec3(1.0, 1.0, 0.99), vec3(0.2, 0.24, 0.36), night), cover * mix(0.8, 0.35, night));
        if (night > 0.5) {
            let g = vec2(az * 1500.0, tz * 800.0);
            let id = floor(g);
            let f = fract(g) - 0.5;
            let h = hash(id);
            let star = step(0.988, h) * smoothstep(0.3, 0.05, length(f)) * (0.55 + 0.45 * sin(t * 1.7 + h * 60.0)) * smoothstep(0.08, 0.35, tz);
            col += vec3(0.9, 0.92, 1.0) * star * (1.0 - cover);
            // the moon
            let moon = normalize(vec3(sin(0.8), cos(0.8), 0.55));
            let m = dot(d, moon);
            col += vec3(0.95, 0.93, 0.85) * (smoothstep(0.99965, 0.9998, m) + pow(max(m, 0.0), 900.0) * 0.25);
        }
    }

    // the skyline, its painted sky fading into ours
    let vf = (FAR_TOP - tz) / (FAR_TOP - FAR_BOT);
    let far = textureSampleGrad(far_tex, samp, vec2(az * FAR_REPEAT, clamp(vf, 0.0, 1.0)), vec2(az_dx * FAR_REPEAT, -tz_dx / (FAR_TOP - FAR_BOT)), vec2(az_dy * FAR_REPEAT, -tz_dy / (FAR_TOP - FAR_BOT))).rgb;
    let in_far = step(0.0, vf) * step(vf, 1.0);
    col = mix(col, far, in_far * smoothstep(0.0, 0.3, vf));
    // the skyline's foot dissolves into the haze over the far city, which deepens toward the
    // rooftops below
    let haze = mix(vec3(0.84, 0.90, 0.92), vec3(0.15, 0.18, 0.29), night);
    let foot = mix(vec3(0.72, 0.76, 0.77), vec3(0.17, 0.20, 0.28), night);
    let under = mix(haze, foot, smoothstep(0.0, NEAR_TOP + 0.08, -tz));
    col = mix(col, haze, in_far * smoothstep(0.62, 1.0, vf) * 0.8);
    col = mix(col, under, step(1.0, vf));

    // birds over the skyline
    col = mix(col, mix(vec3(0.17, 0.21, 0.28), vec3(0.6, 0.64, 0.78), night), birds(d, o.ndc, t) * 0.55);

    // the rooftops below, with traffic down the avenue
    let dn = -tz;
    let vn = (dn - NEAR_TOP) / (NEAR_BOT - NEAR_TOP);
    let un = az * NEAR_REPEAT + 0.37;
    var near = textureSampleGrad(near_tex, samp, vec2(un, clamp(vn, 0.0, 1.0)), vec2(az_dx * NEAR_REPEAT, -tz_dx / (NEAR_BOT - NEAR_TOP)), vec2(az_dy * NEAR_REPEAT, -tz_dy / (NEAR_BOT - NEAR_TOP)));
    let in_near = step(0.0, vn) * step(vn, 1.0);
    {
        // in the uncropped painting's pixels, where the lanes were measured
        let px = vec2(fract(un) * 2600.0, 110.0 + vn * NEAR_ROWS);
        let fwp = max(length(vec2(az_dx * NEAR_REPEAT * 2600.0, tz_dx / (NEAR_BOT - NEAR_TOP) * NEAR_ROWS)), 0.3);
        for (var lane = 0; lane < 2; lane++) {
            let lx = select(0.750, 0.248, lane == 0) * 2600.0;
            for (var i = 0; i < 4; i++) {
                for (var w = 0; w < 2; w++) {
                    let toward = w == 0;
                    let dur = select(11.0, 9.0, toward);
                    var q = fract((t + f32(i) * dur / 4.0 + f32(lane) * 3.0) / dur);
                    if (!toward) {
                        q = 1.0 - q;
                    }
                    let sc = 0.6 + 0.55 * q;
                    let cy = 322.0 * (0.37 + 0.63 * q);
                    let cx = lx + select(4.4, -1.5, toward) * (0.6 + q);
                    let rel = px - vec2(cx, cy);
                    if (night > 0.5) {
                        let r = 1.9 * sc;
                        let lit = clamp(0.5 - (length(rel) - r) / fwp, 0.0, 1.0) + exp(-dot(rel, rel) / (r * r * 6.0)) * 0.5;
                        let c = select(vec3(1.0, 0.3, 0.26), vec3(1.0, 0.94, 0.82), toward);
                        near = vec4(near.rgb + c * lit * near.a, near.a);
                    } else {
                        let cab = box(rel, vec2(1.1, 1.9) * sc, fwp);
                        near = vec4(mix(near.rgb, vec3(0.95, 0.79, 0.3) * near.a, cab), near.a);
                    }
                }
            }
        }
    }
    // the farthest rooftops are hazy too
    let misty = vec4(mix(near.rgb, under * near.a, (1.0 - smoothstep(0.0, 0.5, vn)) * 0.55), near.a);
    col = col * (1.0 - misty.a * in_near) + misty.rgb * in_near;
    col = mix(col, haze, exp(-abs(tz + 0.01) * 16.0) * 0.35);

    // straight down: the street grid far below, through a band of haze as the view steepens
    let deep = smoothstep(NEAR_BOT - 0.26, NEAR_BOT, dn);
    col = mix(col, haze, (1.0 - abs(deep * 2.0 - 1.0)) * 0.55 * step(0.0, -d.z));
    if (d.z < 0.0) {
        var g = ground(gp, gp_dx, gp_dy, t, night);
        g = mix(g, haze, 1.0 - exp(-dist / 700.0));
        col = mix(col, g, deep);
    }

    if (s.lens.w > 0.5) {
        col = to_linear(clamp(col, vec3(0.0), vec3(1.0)));
    }
    return vec4(col, 1.0);
}

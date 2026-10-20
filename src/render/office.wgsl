// One instanced pass for the whole office. Each instance is the unit square mapped by an
// affine transform onto a premultiplied texture.

struct Globals {
    scale: vec2<f32>,
    offset: vec2<f32>,
    srgb_target: f32,
    _pad0: f32,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var color_tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct In {
    @location(0) m: vec4<f32>,
    @location(1) t: vec2<f32>,
    @location(2) glow: vec2<f32>,
    @location(3) uv: vec4<f32>,
    @location(4) tint: vec4<f32>,
};

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec4<f32>,
    @location(2) glow: f32,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32, i: In) -> Out {
    var corners = array<vec2<f32>, 6>(vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 1.0));
    let c = corners[vi];
    let world = vec2(i.m.x * c.x + i.m.z * c.y + i.t.x, i.m.y * c.x + i.m.w * c.y + i.t.y);
    var o: Out;
    o.pos = vec4(world * g.scale + g.offset, 0.0, 1.0);
    o.uv = mix(i.uv.xy, i.uv.zw, c);
    o.tint = i.tint;
    o.glow = i.glow.x;
    return o;
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3(2.4));
    return select(hi, lo, c <= vec3(0.04045));
}

@fragment
fn fs(o: Out) -> @location(0) vec4<f32> {
    let c = textureSample(color_tex, samp, o.uv);
    // premultiplied, in sRGB space like the browser prototype
    var rgb = c.rgb * o.tint.rgb * (1.0 + o.glow);
    let a = c.a * o.tint.a;
    rgb = rgb * o.tint.a;
    if (g.srgb_target > 0.5 && a > 0.0) {
        rgb = to_linear(min(rgb / a, vec3(1.0))) * a;
    }
    return vec4(min(rgb, vec3(a)), a);
}

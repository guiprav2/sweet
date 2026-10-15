struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) css: vec2<f32>,
}

struct LayerUniform {
    // Screen-space corners in physical pixels: TL, TR, BR, BL.
    corners: array<vec4<f32>, 4>,
    opacity: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
    clip: vec4<f32>,
    round: vec4<f32>,
    radii: vec4<f32>,
    viewport: vec2<f32>,
    _pad3: vec2<f32>,
}

@group(0) @binding(0) var layer_tex: texture_2d<f32>;
@group(0) @binding(1) var layer_sampler: sampler;
@group(1) @binding(0) var<uniform> layer: LayerUniform;

fn corner_radius(p: vec2<f32>, box: vec4<f32>, radii: vec4<f32>) -> f32 {
    let right = p.x > box.x + box.z * 0.5;
    let bottom = p.y > box.y + box.w * 0.5;
    if (!right && !bottom) { return radii.x; }
    if (right && !bottom) { return radii.y; }
    if (right && bottom) { return radii.z; }
    return radii.w;
}

fn inside_round(p: vec2<f32>, box: vec4<f32>, radii: vec4<f32>) -> bool {
    if (box.z <= 0.0 || box.w <= 0.0) { return true; }
    if (p.x < box.x || p.y < box.y || p.x >= box.x + box.z || p.y >= box.y + box.w) {
        return false;
    }
    let radius = corner_radius(p, box, radii);
    if (radius < 0.5) { return true; }
    let half = min(box.z, box.w) * 0.5;
    let r = clamp(radius, 0.0, half);
    let center = vec2(
        select(box.x + r, box.x + box.z - r, p.x > box.x + box.z * 0.5),
        select(box.y + r, box.y + box.w - r, p.y > box.y + box.w * 0.5),
    );
    let delta = p - center;
    if (abs(delta.x) <= box.z * 0.5 - r || abs(delta.y) <= box.w * 0.5 - r) {
        return true;
    }
    return dot(delta, delta) <= r * r;
}

@vertex
fn vs(@builtin(vertex_index) index: u32) -> Varyings {
    var out: Varyings;
    let p = layer.corners[index].xy;
    let ndc = vec2(
        p.x / max(layer.viewport.x, 1.0) * 2.0 - 1.0,
        1.0 - p.y / max(layer.viewport.y, 1.0) * 2.0,
    );
    out.position = vec4(ndc, 0.0, 1.0);
    let uvs = array<vec2<f32>, 4>(
        vec2(0.0, 0.0),
        vec2(1.0, 0.0),
        vec2(1.0, 1.0),
        vec2(0.0, 1.0),
    );
    out.uv = uvs[index];
    out.css = p;
    return out;
}

@fragment
fn fs(in: Varyings) -> @location(0) vec4<f32> {
    if (!inside_round(in.css, layer.clip, layer.radii)) {
        discard;
    }
    if (layer.round.z > 0.0 && layer.round.w > 0.0 && !inside_round(in.css, layer.round, layer.radii)) {
        discard;
    }
    let sample = textureSample(layer_tex, layer_sampler, in.uv);
    return vec4(sample.b, sample.g, sample.r, sample.a) * layer.opacity;
}

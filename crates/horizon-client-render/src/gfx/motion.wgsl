struct Temporal {
    current: mat4x4<f32>,
    previous: mat4x4<f32>,
    reproject: mat4x4<f32>,
    jitter: vec4<f32>, // xy: current clip-space jitter; z: reset
}
@group(0) @binding(0) var<uniform> temporal: Temporal;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var atlas: texture_2d<f32>;
@group(0) @binding(3) var atlas_sampler: sampler;
struct Fullscreen { @builtin(position) position: vec4<f32> }
@vertex fn vs_camera(@builtin(vertex_index) i: u32) -> Fullscreen {
    let p = vec2(f32((i << 1u) & 2u), f32(i & 2u));
    return Fullscreen(vec4(p * 2.0 - 1.0, 0.0, 1.0));
}
// Normalized UV displacement, previous minus current, positive Y downward.
@fragment fn fs_camera(in: Fullscreen) -> @location(0) vec2<f32> {
    if temporal.jitter.z != 0.0 { return vec2(0.0); }
    let size = vec2<f32>(textureDimensions(depth));
    let ndc = in.position.xy / size * vec2(2.0, -2.0) + vec2(-1.0, 1.0) - temporal.jitter.xy;
    let z = textureLoad(depth, vec2<i32>(in.position.xy), 0);
    let previous = temporal.reproject * vec4(ndc, z, 1.0);
    if previous.w <= 0.0 { return vec2(0.0); }
    return (previous.xy / previous.w - ndc) * vec2(0.5, -0.5);
}
struct Entity {
    @builtin(position) position: vec4<f32>,
    @location(0) current: vec4<f32>,
    @location(1) previous: vec4<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) alpha: f32,
    @location(4) @interpolate(flat) cull: f32,
}
@vertex fn vs_entity(@location(0) current: vec3<f32>, @location(1) previous: vec3<f32>,
    @location(2) uv: vec2<f32>, @location(3) alpha: f32, @location(4) cull: f32) -> Entity {
    var out: Entity;
    out.current = temporal.current * vec4(current, 1.0);
    out.previous = temporal.previous * vec4(previous, 1.0);
    out.position = out.current;
    out.position.x += temporal.jitter.x * out.position.w;
    out.position.y += temporal.jitter.y * out.position.w;
    out.uv = uv;
    out.alpha = alpha;
    out.cull = cull;
    return out;
}
@fragment fn fs_entity(in: Entity, @builtin(front_facing) front: bool) -> @location(0) vec2<f32> {
    if textureSample(atlas, atlas_sampler, in.uv).a * in.alpha < 0.1 || (in.cull != 0.0 && !front) { discard; }
    if temporal.jitter.z != 0.0 || in.previous.w <= 0.0 { return vec2(0.0); }
    return (in.previous.xy / in.previous.w - in.current.xy / in.current.w) * vec2(0.5, -0.5);
}

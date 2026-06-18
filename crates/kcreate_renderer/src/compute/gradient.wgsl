// Native gradient rasteriser: evaluates a linear or radial colour
// gradient analytically per pixel and writes packed-u32 RGBA
// output. Unlike the blur / levels shaders this pass has no pixel
// input — it synthesises the gradient directly from the stop list,
// so it is a true source-generation pass on the GPU rather than a
// CPU-rasterise-then-upload round-trip.
//
// Colour stops arrive as two parallel storage buffers (offsets and
// straight-alpha RGBA colours), pre-sorted and offset-clamped on
// the host so the shader can assume `stop_offsets` is ascending in
// `[0, 1]`. Interpolation is straight-alpha linear `mix` between
// adjacent stops with `Pad` spread (clamp to the first / last stop
// outside the range), matching the scalar CPU reference in
// `gradient.rs` (`cpu_render_gradient`) within ±1 / 255 per channel.
//
// The bind layout is the shared `FourBuffer` shape used by the
// other compute pipelines (uniform params + two read-only storage
// buffers + one read_write storage buffer), so it reuses
// `build_pipeline` unchanged.

struct GradientParams {
    width: u32,
    height: u32,
    mode: u32,        // 0 = linear, 1 = radial
    stop_count: u32,
    p0: vec2<f32>,    // linear: gradient start (world/local space)
    p1: vec2<f32>,    // linear: gradient end
    center: vec2<f32>,// radial: centre
    radius: f32,      // radial: radius (in the same space as center)
    _pad: f32,
};

@group(0) @binding(0) var<uniform> params: GradientParams;
@group(0) @binding(1) var<storage, read> stop_offsets: array<f32>;
@group(0) @binding(2) var<storage, read> stop_colors: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> output_pixels: array<u32>;

fn pack(c: vec4<f32>) -> u32 {
    let r = u32(clamp(c.r * 255.0 + 0.5, 0.0, 255.0));
    let g = u32(clamp(c.g * 255.0 + 0.5, 0.0, 255.0));
    let b = u32(clamp(c.b * 255.0 + 0.5, 0.0, 255.0));
    let a = u32(clamp(c.a * 255.0 + 0.5, 0.0, 255.0));
    return (r << 24u) | (g << 16u) | (b << 8u) | a;
}

// Fraction along the gradient axis at point `p`, clamped to
// [0, 1] (Pad spread). Degenerate geometry (zero-length axis or
// zero radius) collapses to the first stop.
fn gradient_t(p: vec2<f32>) -> f32 {
    if (params.mode == 0u) {
        let d = params.p1 - params.p0;
        let len2 = dot(d, d);
        if (len2 <= 1e-12) {
            return 0.0;
        }
        return clamp(dot(p - params.p0, d) / len2, 0.0, 1.0);
    }
    if (params.radius <= 1e-6) {
        return 0.0;
    }
    return clamp(length(p - params.center) / params.radius, 0.0, 1.0);
}

// Colour at parameter `t` across the pre-sorted stop list.
fn sample_color(t: f32) -> vec4<f32> {
    let count = params.stop_count;
    if (count <= 1u) {
        return stop_colors[0];
    }
    if (t <= stop_offsets[0]) {
        return stop_colors[0];
    }
    if (t >= stop_offsets[count - 1u]) {
        return stop_colors[count - 1u];
    }
    var i: u32 = 0u;
    loop {
        if (i + 1u >= count) {
            break;
        }
        let o0 = stop_offsets[i];
        let o1 = stop_offsets[i + 1u];
        if (t >= o0 && t <= o1) {
            let denom = o1 - o0;
            if (denom < 1e-6) {
                return stop_colors[i];
            }
            let local = (t - o0) / denom;
            return mix(stop_colors[i], stop_colors[i + 1u], local);
        }
        i = i + 1u;
    }
    return stop_colors[count - 1u];
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x: u32 = gid.x;
    let y: u32 = gid.y;
    if (x >= params.width || y >= params.height) {
        return;
    }
    let idx: u32 = y * params.width + x;
    // Sample at the pixel centre so the ramp is symmetric and
    // matches the scalar CPU reference exactly.
    let p = vec2<f32>(f32(x) + 0.5, f32(y) + 0.5);
    let t = gradient_t(p);
    output_pixels[idx] = pack(sample_color(t));
}

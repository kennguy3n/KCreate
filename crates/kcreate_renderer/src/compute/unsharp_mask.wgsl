// Unsharp-mask compose pass.
//
// Reads packed-u32 RGBA `original` and `blurred` storage buffers
// (the blurred buffer comes from a prior two-pass dispatch of
// gaussian_blur.wgsl) and writes:
//
//     out = original + amount * (original - blurred)
//
// Threshold gates per-channel: if `|original - blurred| < threshold`
// the pixel is passed through. This mirrors Photoshop / GIMP's
// unsharp-mask Threshold slider and keeps gradients un-sharpened.

struct UnsharpParams {
    width: u32,
    height: u32,
    amount: f32,
    threshold: f32,
};

@group(0) @binding(0) var<uniform> params: UnsharpParams;
@group(0) @binding(1) var<storage, read> original_pixels: array<u32>;
@group(0) @binding(2) var<storage, read> blurred_pixels: array<u32>;
@group(0) @binding(3) var<storage, read_write> output_pixels: array<u32>;

fn unpack(p: u32) -> vec4<f32> {
    let r = f32((p >> 24u) & 0xFFu) / 255.0;
    let g = f32((p >> 16u) & 0xFFu) / 255.0;
    let b = f32((p >> 8u) & 0xFFu) / 255.0;
    let a = f32(p & 0xFFu) / 255.0;
    return vec4<f32>(r, g, b, a);
}

fn pack(c: vec4<f32>) -> u32 {
    let r = u32(clamp(c.r * 255.0 + 0.5, 0.0, 255.0));
    let g = u32(clamp(c.g * 255.0 + 0.5, 0.0, 255.0));
    let b = u32(clamp(c.b * 255.0 + 0.5, 0.0, 255.0));
    let a = u32(clamp(c.a * 255.0 + 0.5, 0.0, 255.0));
    return (r << 24u) | (g << 16u) | (b << 8u) | a;
}

fn gated(diff: f32, threshold: f32) -> f32 {
    if (abs(diff) < threshold) {
        return 0.0;
    }
    return diff;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x: u32 = gid.x;
    let y: u32 = gid.y;
    if (x >= params.width || y >= params.height) {
        return;
    }
    let idx: u32 = y * params.width + x;
    let original: vec4<f32> = unpack(original_pixels[idx]);
    let blurred: vec4<f32> = unpack(blurred_pixels[idx]);
    let diff: vec4<f32> = original - blurred;
    let r: f32 = clamp(original.r + params.amount * gated(diff.r, params.threshold), 0.0, 1.0);
    let g: f32 = clamp(original.g + params.amount * gated(diff.g, params.threshold), 0.0, 1.0);
    let b: f32 = clamp(original.b + params.amount * gated(diff.b, params.threshold), 0.0, 1.0);
    let a: f32 = original.a;
    output_pixels[idx] = pack(vec4<f32>(r, g, b, a));
}

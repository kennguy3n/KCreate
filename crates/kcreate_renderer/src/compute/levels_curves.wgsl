// Per-pixel adjustment shader: reads packed-u32 RGBA input,
// applies a 256-entry brightness LUT (uploaded as a storage
// buffer of `f32`), writes packed-u32 RGBA output.
//
// Both Levels (black/white/gamma) and Curves (spline control
// points) compile to the same shape — a 256-entry [0.0, 1.0] LUT
// — on the host before dispatch, so the shader is identical for
// both modes.

struct AdjustParams {
    width: u32,
    height: u32,
    apply_alpha: u32,  // 1 to run the LUT on alpha, 0 to pass through.
    _pad: u32,
};

@group(0) @binding(0) var<uniform> params: AdjustParams;
@group(0) @binding(1) var<storage, read> lut: array<f32>;
@group(0) @binding(2) var<storage, read> input_pixels: array<u32>;
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

fn lookup(channel: f32) -> f32 {
    // Channel arrives in [0.0, 1.0]; quantise to the LUT byte slot.
    let idx_f: f32 = clamp(channel * 255.0, 0.0, 255.0);
    let idx: u32 = u32(idx_f);
    return lut[idx];
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x: u32 = gid.x;
    let y: u32 = gid.y;
    if (x >= params.width || y >= params.height) {
        return;
    }
    let idx: u32 = y * params.width + x;
    let src: vec4<f32> = unpack(input_pixels[idx]);
    var out: vec4<f32>;
    out.r = lookup(src.r);
    out.g = lookup(src.g);
    out.b = lookup(src.b);
    if (params.apply_alpha == 1u) {
        out.a = lookup(src.a);
    } else {
        out.a = src.a;
    }
    output_pixels[idx] = pack(out);
}

// Two-pass separable Gaussian blur compute shader.
//
// I/O is a packed-u32 RGBA storage buffer rather than a storage
// texture so the pipeline does not require any optional wgpu
// adapter feature (downlevel default limits allow read/write
// storage buffers, but `Rgba8Unorm` storage textures need
// `TEXTURE_FORMAT_RGBA_UNORM_STORAGE` which Intel UHD and most
// integrated mobile chips do not expose).
//
// Layout: pixels[i] = (R << 24) | (G << 16) | (B << 8) | A
// Index:  i = y * width + x
//
// Workgroup is 8 x 8 = 64 invocations (well under the downlevel
// 256 ceiling). One dispatch per pass: horizontal first, then
// vertical, with the two passes wired to alternate buffers in
// the bind groups.
//
// The kernel weights are pre-computed on the host (`build_kernel`
// in `mod.rs`) and uploaded once per dispatch.

struct BlurParams {
    radius: u32,
    axis: u32,     // 0 = horizontal, 1 = vertical
    width: u32,
    height: u32,
};

@group(0) @binding(0) var<uniform> params: BlurParams;
@group(0) @binding(1) var<storage, read> kernel_weights: array<f32>;
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

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x: u32 = gid.x;
    let y: u32 = gid.y;
    if (x >= params.width || y >= params.height) {
        return;
    }
    let out_idx: u32 = y * params.width + x;

    if (params.radius == 0u) {
        output_pixels[out_idx] = input_pixels[out_idx];
        return;
    }

    let radius_i: i32 = i32(params.radius);
    var acc: vec4<f32> = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    var weight_sum: f32 = 0.0;

    // Clamp-to-edge boundary handling: keep parity with the CPU
    // reference in `kcreate_raster::filters::gaussian_blur`.
    for (var i: i32 = -radius_i; i <= radius_i; i = i + 1) {
        var sx: i32 = i32(x);
        var sy: i32 = i32(y);
        if (params.axis == 0u) {
            sx = sx + i;
        } else {
            sy = sy + i;
        }
        sx = clamp(sx, 0, i32(params.width) - 1);
        sy = clamp(sy, 0, i32(params.height) - 1);
        let sample_idx: u32 = u32(sy) * params.width + u32(sx);
        let weight: f32 = kernel_weights[u32(i + radius_i)];
        acc = acc + unpack(input_pixels[sample_idx]) * weight;
        weight_sum = weight_sum + weight;
    }

    output_pixels[out_idx] = pack(acc / max(weight_sum, 1e-6));
}

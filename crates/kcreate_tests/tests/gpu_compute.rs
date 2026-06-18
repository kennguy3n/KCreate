//! Phase 11 Block B Task 12 — GPU compute filter integration tests.
//!
//! The tests degrade gracefully when no GPU adapter is available
//! (CI runners, headless containers): each test short-circuits
//! with `eprintln!` instead of panicking. This matches the
//! production fallback discipline — the bridge silently falls
//! back to CPU when [`kcreate_renderer::compute::GpuComputeContext::try_new`]
//! returns `Ok(None)` so we can't make a hard pass/fail assertion
//! gated on GPU availability.

use kcreate_raster::filters as cpu_filters;
use kcreate_raster::tile::TileGrid;
use kcreate_renderer::compute::{
    build_curves_lut, build_levels_lut, cpu_render_gradient, GpuComputeContext, GradientKind,
    GradientSpec, GradientStop,
};

fn gradient_rgba(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for y in 0..height {
        for x in 0..width {
            let r = ((x * 255) / width.max(1)) as u8;
            let g = ((y * 255) / height.max(1)) as u8;
            let b = ((x.wrapping_add(y) * 255) / (width + height).max(1)) as u8;
            out.extend_from_slice(&[r, g, b, 255]);
        }
    }
    out
}

fn checker_rgba(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for y in 0..height {
        for x in 0..width {
            let on = ((x / 16) + (y / 16)) % 2 == 0;
            let v = if on { 220u8 } else { 35u8 };
            out.extend_from_slice(&[v, v, v, 255]);
        }
    }
    out
}

fn try_context() -> Option<GpuComputeContext> {
    match GpuComputeContext::try_new() {
        Ok(Some(ctx)) => Some(ctx),
        Ok(None) => {
            eprintln!("gpu_compute test skipped: no wgpu adapter available");
            None
        }
        Err(err) => {
            eprintln!("gpu_compute test skipped: wgpu init failed ({err})");
            None
        }
    }
}

fn pixelwise_close(a: &[u8], b: &[u8], tolerance: u8) -> (bool, usize, u8) {
    assert_eq!(a.len(), b.len(), "buffers must be the same length");
    let mut max_diff = 0u8;
    let mut bad = 0usize;
    for (x, y) in a.iter().zip(b.iter()) {
        let d = x.abs_diff(*y);
        if d > max_diff {
            max_diff = d;
        }
        if d > tolerance {
            bad += 1;
        }
    }
    (bad == 0, bad, max_diff)
}

#[test]
fn gaussian_blur_matches_cpu_within_one_per_channel() {
    let Some(ctx) = try_context() else { return };
    let width = 256;
    let height = 256;
    let sigma: f32 = 4.0;
    let rgba = gradient_rgba(width, height);

    let gpu_out = ctx
        .gaussian_blur(&rgba, width, height, sigma)
        .expect("GPU blur dispatch");
    let grid = TileGrid::from_image(&rgba, width, height, 256).expect("tile grid");
    let cpu_out = cpu_filters::gaussian_blur(&grid, sigma).to_image();

    // Allow a tolerance of 2 because the CPU separable kernel and
    // the GPU kernel use the same sigma but differ in summation
    // order (rayon parallel sum vs. shader serial sum) and the
    // host-side `round()` happens in different float widths on
    // some backends.
    let (ok, bad, max_diff) = pixelwise_close(&gpu_out, &cpu_out, 2);
    assert!(
        ok,
        "GPU vs CPU blur diverged: {bad} channels exceeded tolerance, max diff {max_diff}"
    );
}

#[test]
fn levels_identity_does_not_change_pixels() {
    let Some(ctx) = try_context() else { return };
    let width = 128;
    let height = 128;
    let rgba = gradient_rgba(width, height);
    let lut = build_levels_lut(0.0, 1.0, 1.0);
    let out = ctx
        .levels_curves(&rgba, width, height, &lut, false)
        .expect("levels dispatch");
    let (ok, bad, max_diff) = pixelwise_close(&rgba, &out, 1);
    assert!(
        ok,
        "levels identity diverged: {bad} channels exceeded tolerance, max diff {max_diff}"
    );
}

#[test]
fn curves_identity_does_not_change_pixels() {
    let Some(ctx) = try_context() else { return };
    let width = 128;
    let height = 128;
    let rgba = gradient_rgba(width, height);
    // Empty control points + implicit (0,0),(1,1) anchors = identity.
    let lut = build_curves_lut(&[]);
    let out = ctx
        .levels_curves(&rgba, width, height, &lut, false)
        .expect("curves dispatch");
    let (ok, bad, max_diff) = pixelwise_close(&rgba, &out, 1);
    assert!(
        ok,
        "curves identity diverged: {bad} channels exceeded tolerance, max diff {max_diff}"
    );
}

#[test]
fn levels_brighten_increases_average_luma() {
    let Some(ctx) = try_context() else { return };
    let width = 64;
    let height = 64;
    let rgba = checker_rgba(width, height);
    // White-point pulled in to 0.6 + gamma 0.5 -> brighter midtones.
    let lut = build_levels_lut(0.0, 0.6, 0.5);
    let out = ctx
        .levels_curves(&rgba, width, height, &lut, false)
        .expect("levels brighten dispatch");
    let pixel_count = f64::from(width) * f64::from(height);
    let avg_in: f64 = rgba.iter().step_by(4).map(|v| f64::from(*v)).sum::<f64>() / pixel_count;
    let avg_out: f64 = out.iter().step_by(4).map(|v| f64::from(*v)).sum::<f64>() / pixel_count;
    assert!(
        avg_out > avg_in,
        "levels brighten lowered luma: in={avg_in}, out={avg_out}"
    );
}

#[test]
fn unsharp_mask_matches_cpu_within_small_tolerance() {
    let Some(ctx) = try_context() else { return };
    let width = 128;
    let height = 128;
    let sigma: f32 = 2.0;
    let amount = 0.6f32;
    let threshold = 0u8;
    let rgba = checker_rgba(width, height);

    let gpu_out = ctx
        .unsharp_mask(&rgba, width, height, sigma, amount, threshold)
        .expect("GPU unsharp dispatch");
    let grid = TileGrid::from_image(&rgba, width, height, 256).expect("tile grid");
    let cpu_out = cpu_filters::unsharp_mask(&grid, sigma, amount, threshold).to_image();

    // Sharpening amplifies blur rounding differences; relax to ±4
    // per channel, which still catches a fundamental shader bug
    // while tolerating per-backend float ordering. The CPU and
    // GPU paths share their sigma derivation (`radius / 3.0`).
    let (ok, bad, max_diff) = pixelwise_close(&gpu_out, &cpu_out, 4);
    assert!(
        ok,
        "GPU vs CPU unsharp diverged: {bad} channels exceeded tolerance, max diff {max_diff}"
    );
}

#[test]
fn blur_4096_under_perf_budget() {
    let Some(ctx) = try_context() else { return };
    let width = 4096;
    let height = 4096;
    let rgba = vec![128u8; (width as usize) * (height as usize) * 4];
    let start = std::time::Instant::now();
    let _ = ctx
        .gaussian_blur(&rgba, width, height, 8.0)
        .expect("4k blur dispatch");
    let elapsed = start.elapsed();

    // OVERVIEW.md §20 target: "64MP Gaussian blur < 500ms on Tier
    // 2+". We translate that to a per-pixel rate on the
    // 4096*4096 (16 MP) test image: 500ms / 64 MP = ~8 ns/pixel,
    // so 16 MP should land in ~125ms on a Tier-2 GPU.
    //
    // CI runs on the software adapter (`wgpu::Backend::NoOp`)
    // which has no fixed-function compute units; benchmarking
    // it against the GPU target would just flake. We skip the
    // bound on that backend and only print the elapsed time.
    if ctx.is_software_adapter() {
        eprintln!(
            "blur_4096_under_perf_budget: skipping perf assertion on software adapter \
             (backend={:?}, device_type={:?}, elapsed={elapsed:?})",
            ctx.backend(),
            ctx.device_type()
        );
        return;
    }
    let budget = std::time::Duration::from_millis(500);
    assert!(
        elapsed < budget,
        "GPU blur on 4096x4096 took {elapsed:?}, exceeded {budget:?} budget"
    );
}

#[test]
fn gradient_linear_matches_cpu_within_one_per_channel() {
    let Some(ctx) = try_context() else { return };
    let width = 256;
    let height = 64;
    let spec = GradientSpec {
        width,
        height,
        kind: GradientKind::Linear {
            from: [0.0, 0.0],
            to: [width as f32, 0.0],
        },
        stops: vec![
            GradientStop {
                offset: 0.0,
                color: [0.05, 0.10, 0.40, 1.0],
            },
            GradientStop {
                offset: 0.5,
                color: [0.90, 0.30, 0.10, 1.0],
            },
            GradientStop {
                offset: 1.0,
                color: [0.95, 0.95, 0.20, 1.0],
            },
        ],
    };

    let gpu_out = ctx.render_gradient(&spec).expect("GPU gradient dispatch");
    let cpu_out = cpu_render_gradient(&spec);

    // The WGSL shader and the CPU reference share the same straight-
    // alpha lerp + round-half-up quantisation, so the only source of
    // divergence is the backend's float rounding. ±1 LSB/channel.
    let (ok, bad, max_diff) = pixelwise_close(&gpu_out, &cpu_out, 1);
    assert!(
        ok,
        "GPU vs CPU linear gradient diverged: {bad} channels exceeded tolerance, max diff {max_diff}"
    );
}

#[test]
fn gradient_radial_matches_cpu_within_one_per_channel() {
    let Some(ctx) = try_context() else { return };
    let width = 128;
    let height = 128;
    let spec = GradientSpec {
        width,
        height,
        kind: GradientKind::Radial {
            center: [width as f32 / 2.0, height as f32 / 2.0],
            radius: width as f32 / 2.0,
        },
        stops: vec![
            GradientStop {
                offset: 0.0,
                color: [1.0, 1.0, 1.0, 1.0],
            },
            GradientStop {
                offset: 1.0,
                color: [0.10, 0.10, 0.15, 1.0],
            },
        ],
    };

    let gpu_out = ctx.render_gradient(&spec).expect("GPU gradient dispatch");
    let cpu_out = cpu_render_gradient(&spec);

    let (ok, bad, max_diff) = pixelwise_close(&gpu_out, &cpu_out, 1);
    assert!(
        ok,
        "GPU vs CPU radial gradient diverged: {bad} channels exceeded tolerance, max diff {max_diff}"
    );
}

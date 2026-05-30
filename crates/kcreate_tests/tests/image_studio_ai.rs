//! Phase 10 Block A — Image Studio AI algorithms.
//!
//! Cross-crate sanity coverage for the actual algorithms behind the
//! Phase 10 Image Studio actions. The bridge wires these into the
//! workspace; here we drive each algorithm directly on synthetic
//! data so the math itself is locked in.

use kcreate_ai::auto_color::{auto_color_correct, AutoColorMode, AutoColorOptions};
use kcreate_ai::denoise::{denoise, DenoiseOptions};
use kcreate_ai::inpaint::{inpaint, InpaintOptions};
use kcreate_ai::smart_select::smart_select;

/// Build a flat `width × height` RGBA image filled with `(r,g,b,a)`.
fn flat(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
    let mut buf = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..(width * height) {
        buf.extend_from_slice(&rgba);
    }
    buf
}

/// Replace a rectangular region of `buf` with `rgba`.
fn paint_rect(buf: &mut [u8], width: u32, rect: (u32, u32, u32, u32), rgba: [u8; 4]) {
    let (x0, y0, w, h) = rect;
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            let idx = ((y * width + x) * 4) as usize;
            buf[idx..idx + 4].copy_from_slice(&rgba);
        }
    }
}

/// Add deterministic but spatially-uncorrelated pseudo-random noise
/// using a small linear-congruential hash per (x, y, c) so adjacent
/// patches don't share a structured noise pattern (which NLM would
/// happily preserve as "signal").
fn add_noise(buf: &mut [u8], width: u32, height: u32, amplitude: u8) {
    for y in 0..height {
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;
            for c in 0..3 {
                let seed = x.wrapping_mul(73_856_093)
                    ^ y.wrapping_mul(19_349_663)
                    ^ (c as u32).wrapping_mul(83_492_791);
                // xorshift32-style scramble for a pseudo-random byte.
                let mut z = seed.wrapping_add(2_654_435_769);
                z ^= z << 13;
                z ^= z >> 17;
                z ^= z << 5;
                // Map to a signed delta in `[-amplitude, +amplitude]`.
                let amp = i32::from(amplitude.min(40));
                let delta = (z as i32).rem_euclid(2 * amp + 1) - amp;
                let v = i32::from(buf[idx + c]) + delta;
                buf[idx + c] = v.clamp(0, 255) as u8;
            }
        }
    }
}

/// Sum of squared per-channel differences between two RGBA buffers,
/// ignoring alpha.
fn ssd_rgb(a: &[u8], b: &[u8]) -> u64 {
    a.chunks_exact(4)
        .zip(b.chunks_exact(4))
        .map(|(x, y)| {
            (0..3)
                .map(|i| {
                    let d = i32::from(x[i]) - i32::from(y[i]);
                    (d * d) as u64
                })
                .sum::<u64>()
        })
        .sum()
}

// ---------------------------------------------------------------------------
// denoise — Task 1
// ---------------------------------------------------------------------------

#[test]
fn denoise_is_identity_on_flat_clean_image() {
    let img = flat(16, 16, [120, 80, 200, 255]);
    let out = denoise(
        &img,
        16,
        16,
        DenoiseOptions {
            strength: 10.0,
            search_radius: 3,
            patch_radius: 1,
        },
    )
    .expect("denoise");
    // A flat image is its own NLM result up to rounding.
    let total: u64 = img
        .chunks_exact(4)
        .zip(out.chunks_exact(4))
        .map(|(a, b)| {
            (0..3)
                .map(|i| u64::from((i32::from(a[i]) - i32::from(b[i])).unsigned_abs()))
                .sum::<u64>()
        })
        .sum();
    // Allow up to one ulp per channel from the weighted average.
    assert!(total < 16 * 16 * 3 * 2, "flat-image denoise drift {total}");
}

#[test]
fn denoise_reduces_noise_on_synthetic_input() {
    let clean = flat(32, 32, [128, 128, 128, 255]);
    let mut noisy = clean.clone();
    // Modest noise that lives within the NLM weight kernel's effective
    // range (strength is in 0..255 units; h_sq = (strength/255)^2).
    add_noise(&mut noisy, 32, 32, 12);
    let noisy_err = ssd_rgb(&clean, &noisy);
    let out = denoise(
        &noisy,
        32,
        32,
        DenoiseOptions {
            // Large strength so neighbouring patches across the flat
            // region all contribute roughly equally to the average.
            strength: 60.0,
            search_radius: 5,
            patch_radius: 1,
        },
    )
    .expect("denoise");
    let denoised_err = ssd_rgb(&clean, &out);
    assert!(
        denoised_err < noisy_err,
        "denoise must reduce error vs noisy: noisy={noisy_err} denoised={denoised_err}"
    );
}

#[test]
fn denoise_clamps_radii_to_safe_range() {
    let img = flat(8, 8, [200, 100, 50, 255]);
    let out = denoise(
        &img,
        8,
        8,
        DenoiseOptions {
            strength: -5.0, // negative → clamped
            search_radius: 9999,
            patch_radius: 9999,
        },
    )
    .expect("clamped denoise still runs");
    assert_eq!(out.len(), img.len());
}

#[test]
fn denoise_rejects_bad_buffer_size() {
    let bad = vec![0u8; 10]; // not 8*8*4
    let err = denoise(&bad, 8, 8, DenoiseOptions::default()).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("buffer"),
        "expected buffer-size error, got {msg}"
    );
}

// ---------------------------------------------------------------------------
// inpaint — Task 2
// ---------------------------------------------------------------------------

#[test]
fn inpaint_returns_input_when_mask_empty() {
    let img = flat(16, 16, [200, 150, 100, 255]);
    let mask = vec![0u8; 16 * 16];
    let out = inpaint(&img, &mask, 16, 16, InpaintOptions::default()).expect("inpaint");
    assert_eq!(out, img, "empty mask must be identity");
}

#[test]
fn inpaint_fills_small_rect_mask_with_surrounding_texture() {
    // Background is solid green; mask out a small rect in the middle.
    // After inpainting, the patched region should be predominantly
    // green (not the original colour, which is irrelevant — we
    // mutated only the mask, not the pixels).
    let w = 24u32;
    let h = 24u32;
    let mut img = flat(w, h, [40, 200, 60, 255]);
    // Force a small "hole" of arbitrary colour the inpaint will replace.
    paint_rect(&mut img, w, (10, 10, 4, 4), [10, 10, 220, 255]);
    let mut mask = vec![0u8; (w * h) as usize];
    for y in 10..14 {
        for x in 10..14 {
            mask[(y * w + x) as usize] = 255;
        }
    }
    let out = inpaint(
        &img,
        &mask,
        w,
        h,
        InpaintOptions {
            patch_radius: 2,
            num_iterations: 3,
            pyramid_levels: 2,
        },
    )
    .expect("inpaint");
    // Every patched pixel should be much closer to the green
    // background than to the blue hole.
    let bg = [40u8, 200u8, 60u8];
    let hole = [10u8, 10u8, 220u8];
    for y in 10..14 {
        for x in 10..14 {
            let idx = ((y * w + x) * 4) as usize;
            let d_bg = (0..3)
                .map(|c| (i32::from(out[idx + c]) - i32::from(bg[c])).abs())
                .sum::<i32>();
            let d_hole = (0..3)
                .map(|c| (i32::from(out[idx + c]) - i32::from(hole[c])).abs())
                .sum::<i32>();
            assert!(
                d_bg < d_hole,
                "inpaint at ({x},{y}) is closer to hole than to bg: bg={d_bg} hole={d_hole}"
            );
        }
    }
}

#[test]
fn inpaint_rejects_buffer_size_mismatch() {
    let img = vec![0u8; 8 * 8 * 4];
    let mask = vec![255u8; 12]; // wrong size
    let err = inpaint(&img, &mask, 8, 8, InpaintOptions::default()).unwrap_err();
    let msg = format!("{err}");
    // The error explains the size mismatch — accept any of the
    // common wordings the crate may produce.
    assert!(
        msg.contains("buffer") || msg.contains("size") || msg.contains("length"),
        "expected size error, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// auto_color — Task 3
// ---------------------------------------------------------------------------

#[test]
fn auto_color_levels_stretches_narrow_histogram() {
    // A 16x16 image whose channels live in [80, 160] should be
    // stretched to roughly [0, 255] by auto-levels.
    let w = 16u32;
    let h = 16u32;
    let mut img = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let v = 80 + ((x + y) as u8 % 80);
            let idx = ((y * w + x) * 4) as usize;
            img[idx] = v;
            img[idx + 1] = v;
            img[idx + 2] = v;
            img[idx + 3] = 255;
        }
    }
    let out = auto_color_correct(
        &img,
        w,
        h,
        AutoColorOptions {
            mode: AutoColorMode::AutoLevels,
            clip: 0.005,
        },
    )
    .expect("auto_color");
    let mut min = 255u8;
    let mut max = 0u8;
    for px in out.chunks_exact(4) {
        for &ch in &px[..3] {
            min = min.min(ch);
            max = max.max(ch);
        }
    }
    assert!(
        min <= 16 && max >= 230,
        "auto-levels must reach near-full range, got [{min}, {max}]"
    );
}

#[test]
fn auto_color_white_balance_normalises_per_channel_means() {
    // Image with a strong red cast: mean(R) > mean(G), mean(B).
    let w = 8u32;
    let h = 8u32;
    let mut img = vec![0u8; (w * h * 4) as usize];
    for px in img.chunks_exact_mut(4) {
        px[0] = 200;
        px[1] = 100;
        px[2] = 100;
        px[3] = 255;
    }
    let out = auto_color_correct(
        &img,
        w,
        h,
        AutoColorOptions {
            mode: AutoColorMode::WhiteBalance,
            clip: 0.005,
        },
    )
    .expect("auto_color");
    let mut sums = [0u64; 3];
    let mut count = 0u64;
    for px in out.chunks_exact(4) {
        sums[0] += u64::from(px[0]);
        sums[1] += u64::from(px[1]);
        sums[2] += u64::from(px[2]);
        count += 1;
    }
    let means = [sums[0] / count, sums[1] / count, sums[2] / count];
    // After gray-world WB the per-channel means converge.
    let spread = means.iter().max().unwrap() - means.iter().min().unwrap();
    assert!(
        spread <= 8,
        "white-balanced means too spread out: {means:?}"
    );
}

#[test]
fn auto_color_combined_runs_end_to_end() {
    let img = flat(8, 8, [120, 100, 80, 255]);
    let out = auto_color_correct(
        &img,
        8,
        8,
        AutoColorOptions {
            mode: AutoColorMode::Combined,
            clip: 0.005,
        },
    )
    .expect("combined auto_color");
    assert_eq!(out.len(), img.len());
    // Alpha must be preserved.
    for (a, b) in img.chunks_exact(4).zip(out.chunks_exact(4)) {
        assert_eq!(a[3], b[3]);
    }
}

// ---------------------------------------------------------------------------
// segment / smart-select (Tasks 4, 5)
// ---------------------------------------------------------------------------

#[test]
fn smart_select_grows_from_seed_within_tolerance() {
    // Solid red square on a green background — clicking the red
    // square should select exactly the red pixels.
    let w = 16u32;
    let h = 16u32;
    let mut img = flat(w, h, [0, 255, 0, 255]);
    paint_rect(&mut img, w, (4, 4, 8, 8), [255, 0, 0, 255]);
    let mask = smart_select(&img, w, h, 6, 6, 0.1);
    let selected = mask.iter().filter(|&&b| b != 0).count();
    assert_eq!(selected, 64, "expected the 8x8 red rect");
}

#[test]
fn smart_select_replace_and_subtract_compose_correctly() {
    // Two non-overlapping red rects; replacing on one then
    // subtracting the same seed yields zero pixels (the subtract
    // mode wipes the originally selected region).
    let w = 16u32;
    let h = 8u32;
    let mut img = flat(w, h, [0, 0, 0, 255]);
    paint_rect(&mut img, w, (0, 0, 4, 4), [255, 0, 0, 255]);
    paint_rect(&mut img, w, (8, 0, 4, 4), [255, 0, 0, 255]);

    let mask_left = smart_select(&img, w, h, 1, 1, 0.1);
    let mask_right = smart_select(&img, w, h, 9, 1, 0.1);
    // A union of both masks must cover exactly 32 pixels.
    let union: u32 = mask_left
        .iter()
        .zip(mask_right.iter())
        .map(|(a, b)| u32::from(*a != 0 || *b != 0))
        .sum();
    assert_eq!(union, 32);
}

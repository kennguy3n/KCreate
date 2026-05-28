//! Color range selection — pick every pixel within a perceptual
//! distance of a target colour.
//!
//! Distance is computed in CIE Lab space using CIE76 ΔE (Euclidean
//! distance in Lab). This gives a perceptually-uniform "fuzziness"
//! control — a value of `0.10` picks colours indistinguishable from
//! the target to most viewers; `0.30` is "noticeably similar";
//! `0.60` is "in the same family".
//!
//! Returned mask is a `Vec<bool>` of length `width * height` so the
//! caller can OR it with other masks (smart-select, manual paint,
//! existing alpha channel) without an extra decode step. The bridge
//! layer serialises it to a compact byte mask for IPC; passing
//! around `Vec<bool>` here keeps the in-process API ergonomic.

use rayon::prelude::*;

/// Maximum meaningful ΔE in CIE76. Lab's L channel runs `[0, 100]`
/// and a-b channels run roughly `[-128, 128]`. A ΔE > 100 is
/// "totally different colour" by any reasonable measure, so we
/// scale the user-facing `fuzziness` slider linearly across
/// `[0, 100]`.
const MAX_LAB_DELTA_E: f64 = 100.0;

/// Build a selection mask containing every pixel whose colour is
/// within `fuzziness * MAX_LAB_DELTA_E` of `target_color` in CIE76
/// Lab space. Operates on a flat RGBA8 buffer; alpha is ignored
/// for distance but fully-transparent pixels are never selected.
///
/// The mask is row-parallel via rayon. Empty / mismatched input
/// returns an empty mask sized to the requested dimensions so the
/// caller can rely on `mask.len() == width * height`.
#[must_use]
pub fn select_by_color_range(
    image_data: &[u8],
    width: u32,
    height: u32,
    target_color: [u8; 4],
    fuzziness: f64,
) -> Vec<bool> {
    let total = (width as usize) * (height as usize);
    if total == 0 || image_data.len() != total * 4 {
        return vec![false; total];
    }
    let fuzz = fuzziness.clamp(0.0, 1.0);
    let target_lab = rgb_to_lab(target_color[0], target_color[1], target_color[2]);
    let threshold = fuzz * MAX_LAB_DELTA_E;
    // Squared comparison avoids a sqrt per pixel.
    let threshold_sq = threshold * threshold;

    image_data
        .par_chunks_exact(4)
        .map(|px| {
            if px[3] == 0 {
                return false;
            }
            let lab = rgb_to_lab(px[0], px[1], px[2]);
            let dl = lab.0 - target_lab.0;
            let da = lab.1 - target_lab.1;
            let db = lab.2 - target_lab.2;
            let d_sq = dl * dl + da * da + db * db;
            d_sq <= threshold_sq
        })
        .collect()
}

/// Pack a boolean mask into a dense byte buffer (`255` selected,
/// `0` unselected). The bridge uses this on the way out to the
/// renderer; algorithms that consume the mask in-process should
/// keep using the `Vec<bool>` form.
#[must_use]
pub fn pack_mask(mask: &[bool]) -> Vec<u8> {
    mask.par_iter()
        .map(|b| if *b { 255u8 } else { 0u8 })
        .collect()
}

/// Convert sRGB → Lab via the standard D65 white-point pipeline
/// (sRGB → linear → CIE XYZ → Lab). Returns `(L*, a*, b*)`.
///
/// `L*` is in `[0, 100]`; `a*` / `b*` are unbounded in principle but
/// fall in roughly `[-128, 128]` for sRGB inputs.
#[allow(clippy::many_single_char_names)]
fn rgb_to_lab(r: u8, g: u8, b: u8) -> (f64, f64, f64) {
    let r = srgb_to_linear(f64::from(r) / 255.0);
    let g = srgb_to_linear(f64::from(g) / 255.0);
    let b = srgb_to_linear(f64::from(b) / 255.0);

    // sRGB D65 → XYZ (Bradford-adapted) matrix.
    let x = 0.412_390_799_265_959_3 * r + 0.357_584_339_383_877_9 * g + 0.180_480_788_401_834_2 * b;
    let y =
        0.212_639_005_871_510_4 * r + 0.715_168_678_767_756_2 * g + 0.072_192_315_360_733_43 * b;
    let z =
        0.019_330_818_715_591_85 * r + 0.119_194_779_794_625_99 * g + 0.950_532_152_249_660_7 * b;

    // D65 reference white.
    let xn = 0.950_47;
    let yn = 1.0;
    let zn = 1.088_83;

    let fx = lab_f(x / xn);
    let fy = lab_f(y / yn);
    let fz = lab_f(z / zn);

    let l = 116.0 * fy - 16.0;
    let a = 500.0 * (fx - fy);
    let b_lab = 200.0 * (fy - fz);
    (l, a, b_lab)
}

fn lab_f(t: f64) -> f64 {
    // CIE 1976 L*a*b* helper. The cutoff and linear segment match
    // the official definition (6/29)^3 and (1/3)*(29/6)^2.
    const DELTA: f64 = 6.0 / 29.0;
    if t > DELTA * DELTA * DELTA {
        t.cbrt()
    } else {
        t / (3.0 * DELTA * DELTA) + 4.0 / 29.0
    }
}

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            buf.extend_from_slice(&rgba);
        }
        buf
    }

    #[test]
    fn exact_match_selects_all_with_zero_fuzz() {
        let buf = solid(4, 4, [200, 50, 50, 255]);
        let mask = select_by_color_range(&buf, 4, 4, [200, 50, 50, 255], 0.0);
        assert_eq!(mask.len(), 16);
        // Floor of ΔE on identical sRGB triples can be a few
        // hundredths of a unit when the gamma path runs in `f64`,
        // so a zero-fuzz selection still picks an exact match.
        assert!(
            mask.iter().all(|b| *b),
            "exact-match zero-fuzz failed to select identical pixels"
        );
    }

    #[test]
    fn mismatched_color_zero_fuzz_selects_nothing() {
        let buf = solid(4, 4, [200, 50, 50, 255]);
        let mask = select_by_color_range(&buf, 4, 4, [10, 200, 200, 255], 0.0);
        assert!(mask.iter().all(|b| !*b));
    }

    #[test]
    fn larger_fuzziness_selects_more() {
        let mut buf = Vec::with_capacity(4 * 4 * 4);
        for i in 0..16u8 {
            buf.extend_from_slice(&[200u8.saturating_sub(i), 50, 50, 255]);
        }
        let m_low = select_by_color_range(&buf, 4, 4, [200, 50, 50, 255], 0.05);
        let m_high = select_by_color_range(&buf, 4, 4, [200, 50, 50, 255], 0.30);
        let low_count = m_low.iter().filter(|b| **b).count();
        let high_count = m_high.iter().filter(|b| **b).count();
        assert!(
            high_count > low_count,
            "fuzziness sweep failed: low={low_count} high={high_count}"
        );
    }

    #[test]
    fn transparent_pixels_never_selected() {
        let buf = solid(4, 4, [200, 50, 50, 0]);
        let mask = select_by_color_range(&buf, 4, 4, [200, 50, 50, 0], 1.0);
        assert!(mask.iter().all(|b| !*b));
    }

    #[test]
    fn pack_mask_round_trips_through_byte_form() {
        let mask = vec![true, false, true, true, false, false];
        let bytes = pack_mask(&mask);
        assert_eq!(bytes, vec![255, 0, 255, 255, 0, 0]);
    }

    #[test]
    fn empty_buffer_returns_empty_mask() {
        let mask = select_by_color_range(&[], 0, 0, [0; 4], 0.5);
        assert!(mask.is_empty());
    }

    #[test]
    fn mismatched_buffer_size_returns_empty_mask() {
        let buf = vec![0u8; 5];
        let mask = select_by_color_range(&buf, 4, 4, [0; 4], 0.5);
        assert_eq!(mask.len(), 16);
        assert!(mask.iter().all(|b| !*b));
    }
}

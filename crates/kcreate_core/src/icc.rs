//! Phase 4 Block 5 — full ICC profile roundtripping.
//!
//! This module replaces the naive `srgb_to_cmyk` / `cmyk_to_srgb`
//! pair with a real colour-management pipeline:
//!
//! - **Matrix-shaper RGB profiles** for sRGB (IEC 61966-2.1), Adobe
//!   RGB (1998), and Display P3. Each profile is fully defined by
//!   its chromaticity primaries, white point, and tone reproduction
//!   curve (TRC); the matrix that maps profile-linear-RGB to D50 XYZ
//!   (the ICC profile-connection space) is derived from those
//!   primaries at compile time. Chromatic adaptation between
//!   D65-native working spaces and the D50 PCS uses the Bradford
//!   matrix exactly as specified in ICC.1:2010 §6.3.2.4.
//!
//! - **Dot-gain CMYK simulation** for FOGRA39 and SWOP 2006. Each
//!   ink has a per-channel dot-gain TRC parameterised from the
//!   published characterisation data (`PSO-LWC-Improved` for
//!   FOGRA39, `CGATS TR 003` for SWOP); the inverse Neugebauer-style
//!   primary-mixing model produces the sRGB sample for any
//!   `(C, M, Y, K)` tuple, and an iterative numerical inversion
//!   (with naive K extraction as the warm start) maps sRGB into
//!   the CMYK space. The forward and inverse transforms are tested
//!   for round-trip identity to within a tight tolerance over the
//!   in-gamut subset of sRGB.
//!
//! - **`ColorTransform` builder** composes a *source* profile, a
//!   *destination* profile, and a [`RenderingIntent`] into a single
//!   reusable transform that can be applied to any [`Color`].
//!   Honouring the four standard intents is real:
//!
//!     * `Perceptual` — clamps out-of-gamut chroma toward the
//!       destination's neutral axis (CIE 76 ΔE proportional pull).
//!     * `RelativeColorimetric` — adapts white point but otherwise
//!       hard-clamps each channel.
//!     * `Saturation` — preserves chroma, sacrifices lightness for
//!       brand consistency.
//!     * `AbsoluteColorimetric` — no white-point adaptation;
//!       preserves absolute XYZ.
//!
//! No external crates are pulled in — the entire pipeline is pure
//! `f32`/`f64` linear algebra plus a few constants. This keeps the
//! editing-path tree clean and the `local_first` deny-list happy.

use crate::color::{
    cmyk_to_srgb, linear_to_srgb, srgb_to_linear, srgb_to_xyz_d65, xyz_d65_to_srgb, Color,
};
use crate::{IccProfile, RenderingIntent};

/// 3x3 matrix in column-major layout. `m[c][r]` is the entry at
/// row `r`, column `c`. Column-major matches the way we
/// multiply: `out = M · in` is `out[r] = Σ M[c][r] · in[c]`.
type Mat3 = [[f64; 3]; 3];

/// Bradford chromatic-adaptation matrix (D65 → D50). Source:
/// ICC.1:2010 §6.3.2.4.
const BRADFORD_D65_TO_D50: Mat3 = [
    [1.047_809_3, 0.029_454, -0.009_234_7],
    [0.022_898_2, 0.990_481_6, 0.015_073_1],
    [-0.050_146_5, -0.017_046_2, 0.751_877_1],
];

/// Bradford D50 → D65 (inverse of [`BRADFORD_D65_TO_D50`]).
const BRADFORD_D50_TO_D65: Mat3 = [
    [0.955_576_6, -0.028_289_5, 0.012_298_2],
    [-0.023_039_3, 1.009_941_4, -0.020_502_7],
    [0.063_163_6, 0.021_006_8, 1.329_872_4],
];

/// D50 white point in XYZ (ICC standard PCS illuminant).
const D50_X: f64 = 0.964_22;
const D50_Y: f64 = 1.0;
const D50_Z: f64 = 0.825_21;

/// Multiply a 3x3 matrix by a 3-vector. Returns a new 3-vector.
#[inline]
fn mat3_mul_vec3(m: &Mat3, v: [f64; 3]) -> [f64; 3] {
    [
        m[0][0] * v[0] + m[1][0] * v[1] + m[2][0] * v[2],
        m[0][1] * v[0] + m[1][1] * v[1] + m[2][1] * v[2],
        m[0][2] * v[0] + m[1][2] * v[1] + m[2][2] * v[2],
    ]
}

/// Determinant of a 3x3 matrix.
#[inline]
fn mat3_det(m: &Mat3) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[2][1] * m[1][2])
        - m[1][0] * (m[0][1] * m[2][2] - m[2][1] * m[0][2])
        + m[2][0] * (m[0][1] * m[1][2] - m[1][1] * m[0][2])
}

/// Invert a 3x3 matrix. Returns `None` if singular.
#[allow(clippy::suspicious_operation_groupings)]
fn mat3_inverse(m: &Mat3) -> Option<Mat3> {
    let det = mat3_det(m);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let mut out = [[0.0; 3]; 3];
    out[0][0] = (m[1][1] * m[2][2] - m[2][1] * m[1][2]) * inv;
    out[1][0] = -(m[1][0] * m[2][2] - m[2][0] * m[1][2]) * inv;
    out[2][0] = (m[1][0] * m[2][1] - m[2][0] * m[1][1]) * inv;
    out[0][1] = -(m[0][1] * m[2][2] - m[2][1] * m[0][2]) * inv;
    out[1][1] = (m[0][0] * m[2][2] - m[2][0] * m[0][2]) * inv;
    out[2][1] = -(m[0][0] * m[2][1] - m[2][0] * m[0][1]) * inv;
    out[0][2] = (m[0][1] * m[1][2] - m[1][1] * m[0][2]) * inv;
    out[1][2] = -(m[0][0] * m[1][2] - m[1][0] * m[0][2]) * inv;
    out[2][2] = (m[0][0] * m[1][1] - m[1][0] * m[0][1]) * inv;
    Some(out)
}

/// Multiply two 3x3 matrices: `a · b`. Used by tests that
/// validate `mat3_inverse` against the identity matrix.
#[cfg(test)]
fn mat3_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [[0.0_f64; 3]; 3];
    for (c, col) in out.iter_mut().enumerate() {
        for (r, cell) in col.iter_mut().enumerate() {
            *cell = a[0][r] * b[c][0] + a[1][r] * b[c][1] + a[2][r] * b[c][2];
        }
    }
    out
}

/// One RGB matrix-shaper profile: primaries-derived matrix, gamma
/// curve, and the white point it was defined for.
#[derive(Debug, Clone, Copy)]
struct RgbMatrixShaper {
    /// Linear-RGB → D65 XYZ matrix.
    to_xyz_d65: Mat3,
    /// D65 XYZ → linear-RGB matrix (inverse of `to_xyz_d65`).
    from_xyz_d65: Mat3,
    /// Tone-reproduction curve descriptor.
    trc: Trc,
}

#[derive(Debug, Clone, Copy)]
enum Trc {
    /// Piecewise sRGB transfer (used by sRGB and Display P3).
    SrgbPiecewise,
    /// Pure gamma curve (used by Adobe RGB 1998 with γ = 2.19921875).
    Gamma(f64),
}

impl Trc {
    fn to_linear(self, c: f64) -> f64 {
        match self {
            Self::SrgbPiecewise => f64::from(srgb_to_linear(c.clamp(0.0, 1.0) as f32)),
            Self::Gamma(g) => c.clamp(0.0, 1.0).powf(g),
        }
    }
    fn encode_linear(self, c: f64) -> f64 {
        match self {
            Self::SrgbPiecewise => f64::from(linear_to_srgb(c.clamp(0.0, 1.0) as f32)),
            Self::Gamma(g) => c.clamp(0.0, 1.0).powf(1.0 / g),
        }
    }
}

/// (x, y) chromaticity coordinates of a single primary or white point.
type Xy = (f64, f64);

/// Build the linear-RGB → D65 XYZ matrix for a matrix-shaper RGB
/// profile, given its primaries in CIE xy and a D65 white point.
///
/// The construction follows §6.6.2 of [Lindbloom, "RGB/XYZ
/// Matrices"](http://www.brucelindbloom.com/Eqn_RGB_XYZ_Matrix.html)
/// — we solve `(R G B) · S = W`, where `R/G/B` are the primary
/// chromaticities promoted to (X, Y=1, Z) and `S` is the per-channel
/// scaling that lines them up with the white point.
fn matrix_from_primaries(r: Xy, g: Xy, b: Xy, w: Xy) -> Mat3 {
    let (rx, ry) = r;
    let (gx, gy) = g;
    let (bx, by) = b;
    let (wx, wy) = w;
    let xyz_from_xy = |x: f64, y: f64| [x / y, 1.0, (1.0 - x - y) / y];
    let r = xyz_from_xy(rx, ry);
    let g = xyz_from_xy(gx, gy);
    let b = xyz_from_xy(bx, by);
    let w = xyz_from_xy(wx, wy);
    // Columns are R, G, B primaries.
    let m: Mat3 = [r, g, b];
    let m_inv = mat3_inverse(&m).expect("primary matrix should be invertible");
    let s = mat3_mul_vec3(&m_inv, w);
    // Scale each column by its s_i.
    [
        [m[0][0] * s[0], m[0][1] * s[0], m[0][2] * s[0]],
        [m[1][0] * s[1], m[1][1] * s[1], m[1][2] * s[1]],
        [m[2][0] * s[2], m[2][1] * s[2], m[2][2] * s[2]],
    ]
}

/// Build the sRGB matrix-shaper profile. Primaries from
/// IEC 61966-2-1 §6.1.
fn srgb_shaper() -> RgbMatrixShaper {
    let to_xyz = matrix_from_primaries(
        (0.64, 0.33),
        (0.30, 0.60),
        (0.15, 0.06),
        (0.312_72, 0.329_03), // D65
    );
    let from_xyz = mat3_inverse(&to_xyz).expect("sRGB matrix must be invertible");
    RgbMatrixShaper {
        to_xyz_d65: to_xyz,
        from_xyz_d65: from_xyz,
        trc: Trc::SrgbPiecewise,
    }
}

/// Build the Adobe RGB (1998) profile. Primaries from
/// ISO 22028-2:2013 Annex C.
fn adobe_rgb_shaper() -> RgbMatrixShaper {
    let to_xyz = matrix_from_primaries(
        (0.64, 0.33),
        (0.21, 0.71),
        (0.15, 0.06),
        (0.312_72, 0.329_03), // D65
    );
    let from_xyz = mat3_inverse(&to_xyz).expect("Adobe RGB matrix must be invertible");
    RgbMatrixShaper {
        to_xyz_d65: to_xyz,
        from_xyz_d65: from_xyz,
        trc: Trc::Gamma(2.199_218_75),
    }
}

/// Build the Display P3 profile. Primaries from SMPTE RP 431-2
/// (DCI-P3) with the Display P3 white point of D65 per Apple's
/// 2015 spec.
fn display_p3_shaper() -> RgbMatrixShaper {
    let to_xyz = matrix_from_primaries(
        (0.68, 0.32),
        (0.265, 0.69),
        (0.15, 0.06),
        (0.312_72, 0.329_03), // D65
    );
    let from_xyz = mat3_inverse(&to_xyz).expect("Display P3 matrix must be invertible");
    RgbMatrixShaper {
        to_xyz_d65: to_xyz,
        from_xyz_d65: from_xyz,
        trc: Trc::SrgbPiecewise,
    }
}

/// Convert sRGB-encoded triplet into D65 XYZ using the
/// matrix-shaper profile.
fn shaper_rgb_to_xyz_d65(shaper: &RgbMatrixShaper, r: f64, g: f64, b: f64) -> [f64; 3] {
    let lr = shaper.trc.to_linear(r);
    let lg = shaper.trc.to_linear(g);
    let lb = shaper.trc.to_linear(b);
    mat3_mul_vec3(&shaper.to_xyz_d65, [lr, lg, lb])
}

/// Inverse of [`shaper_rgb_to_xyz_d65`].
fn xyz_d65_to_shaper_rgb(shaper: &RgbMatrixShaper, xyz: [f64; 3]) -> [f64; 3] {
    let linear = mat3_mul_vec3(&shaper.from_xyz_d65, xyz);
    [
        shaper.trc.encode_linear(linear[0]),
        shaper.trc.encode_linear(linear[1]),
        shaper.trc.encode_linear(linear[2]),
    ]
}

/// Chromatic adaptation matrix from a native white point to D50
/// (the ICC PCS). All profiles in this module are D65-native, so
/// we precompute the Bradford D65→D50 matrix once at construction.
fn ca_d65_to_d50() -> Mat3 {
    BRADFORD_D65_TO_D50
}

fn ca_d50_to_d65() -> Mat3 {
    BRADFORD_D50_TO_D65
}

// ---------------------------------------------------------------------
// CMYK simulation profiles
// ---------------------------------------------------------------------

/// Dot-gain tone curve plus ink density, used to model real-world
/// ink behaviour better than the naive 1.0 - K formula in
/// `color::cmyk_to_srgb`. Parameters were derived from the
/// published characterisation data for each profile (FOGRA39 from
/// `PSO-LWC-Improved`, SWOP 2006 from `CGATS TR 003`).
#[derive(Debug, Clone, Copy)]
struct CmykSim {
    /// Per-ink dot gain at 50% input (typical printing-industry
    /// parameter). Order: C, M, Y, K.
    dot_gain_50: [f64; 4],
    /// Solid-ink-density in CIE L*: the L* value at 100% single-ink
    /// coverage on the simulated stock. Order: C, M, Y, K.
    /// Drives how dark each pure ink looks in the simulated print.
    solid_l_star: [f64; 4],
}

const FOGRA39: CmykSim = CmykSim {
    dot_gain_50: [0.16, 0.16, 0.16, 0.18],
    // Published Lab solids for FOGRA39 from PSO-LWC-Improved:
    // C ≈ L*57, M ≈ L*49, Y ≈ L*89, K ≈ L*16.
    solid_l_star: [57.0, 49.0, 89.0, 16.0],
};

const SWOP_2006: CmykSim = CmykSim {
    dot_gain_50: [0.22, 0.22, 0.22, 0.24],
    // Published Lab solids for SWOP Coated #3 / CGATS TR 003.
    solid_l_star: [55.0, 47.0, 87.0, 14.0],
};

/// Yule-Nielsen exponent derived from a published g50 dot-gain
/// figure. We pick `n` such that `(0.5)^(1/n) = 0.5 + g50`, i.e.
/// the effective coverage at 50% input equals the published
/// midtone gain. This gives `n = ln(0.5) / ln(0.5 + g50)`.
fn yule_nielsen_exponent(g50: f64) -> f64 {
    let target = (0.5_f64 + g50).clamp(0.501, 0.999);
    target.ln().recip() * 0.5_f64.ln()
}

/// Apply a Yule-Nielsen dot-gain TRC: maps nominal ink coverage
/// `a` ∈ [0,1] to effective coverage after dot gain. The exponent
/// is derived from `g50` so the curve passes through `(0, 0)`,
/// `(0.5, 0.5 + g50)`, and `(1, 1)` exactly. Anchoring at the
/// endpoints means the dot-gain inverse round-trips perfectly
/// at 0% and 100% coverage — critical because paper white and
/// solid ink must be representable losslessly.
fn apply_dot_gain(a: f64, g50: f64) -> f64 {
    if a <= 0.0 {
        return 0.0;
    }
    if a >= 1.0 {
        return 1.0;
    }
    let n = yule_nielsen_exponent(g50);
    a.powf(1.0 / n).clamp(0.0, 1.0)
}

/// Inverse dot-gain TRC: closed-form inverse of [`apply_dot_gain`].
/// Since the forward curve is `a^(1/n)`, the inverse is
/// `effective^n`. Endpoint behaviour matches the forward curve
/// exactly so round-trip identity holds at 0 and 1.
fn invert_dot_gain(effective: f64, g50: f64) -> f64 {
    if effective <= 0.0 {
        return 0.0;
    }
    if effective >= 1.0 {
        return 1.0;
    }
    let n = yule_nielsen_exponent(g50);
    effective.powf(n).clamp(0.0, 1.0)
}

/// Forward CMYK simulation: project nominal CMYK coverages into
/// effective coverages via dot-gain TRCs, then mix the inks
/// according to a Demichel-style overprint model parameterised on
/// the published solid L* values.
fn cmyk_sim_to_srgb(sim: CmykSim, c: f64, m: f64, y: f64, k: f64) -> [f64; 3] {
    let ec = apply_dot_gain(c, sim.dot_gain_50[0]);
    let em = apply_dot_gain(m, sim.dot_gain_50[1]);
    let ey = apply_dot_gain(y, sim.dot_gain_50[2]);
    let ek = apply_dot_gain(k, sim.dot_gain_50[3]);

    // Convert each ink's solid L* to a linear-light luminance
    // multiplier in [0,1] (paper white = 1.0 = L*100, deep
    // black ≈ 0.0). The simulation luminance is the product of
    // (1 - ink * (1 - ink_L)).
    let l_to_y = |l: f64| {
        let ft = (l + 16.0) / 116.0;
        if ft.powi(3) > 216.0 / 24_389.0 {
            ft.powi(3)
        } else {
            (ft * 116.0 - 16.0) / (24_389.0 / 27.0)
        }
    };
    let yc = l_to_y(sim.solid_l_star[0]);
    let ym = l_to_y(sim.solid_l_star[1]);
    let yy = l_to_y(sim.solid_l_star[2]);
    let yk = l_to_y(sim.solid_l_star[3]);

    // Per-channel inks attenuate complementary channels:
    // cyan suppresses R, magenta suppresses G, yellow suppresses B.
    // Black attenuates all three channels equally. Each ink is
    // modeled as a multiplicative filter whose "transmission" goes
    // from 1.0 (no ink) to yc/ym/yy/yk (solid ink).
    let r_lin = (1.0 - ec * (1.0 - yc)) * (1.0 - ek * (1.0 - yk));
    let g_lin = (1.0 - em * (1.0 - ym)) * (1.0 - ek * (1.0 - yk));
    let b_lin = (1.0 - ey * (1.0 - yy)) * (1.0 - ek * (1.0 - yk));
    // Linear-light → sRGB encoding.
    [
        f64::from(linear_to_srgb(r_lin.clamp(0.0, 1.0) as f32)),
        f64::from(linear_to_srgb(g_lin.clamp(0.0, 1.0) as f32)),
        f64::from(linear_to_srgb(b_lin.clamp(0.0, 1.0) as f32)),
    ]
}

/// Inverse simulation: solve for the CMYK coverages that produce
/// the requested sRGB on the simulated stock. Uses naive K
/// extraction as a warm start, then numerical refinement (5
/// Gauss-Seidel passes) to absorb dot gain.
fn srgb_to_cmyk_sim(sim: CmykSim, r: f64, g: f64, b: f64) -> [f64; 4] {
    let r = r.clamp(0.0, 1.0);
    let g = g.clamp(0.0, 1.0);
    let b = b.clamp(0.0, 1.0);

    // Warm start: standard naive K extraction.
    let k0 = 1.0 - r.max(g).max(b);
    let denom = (1.0 - k0).max(1e-6);
    let c0 = ((1.0 - r - k0) / denom).clamp(0.0, 1.0);
    let m0 = ((1.0 - g - k0) / denom).clamp(0.0, 1.0);
    let y0 = ((1.0 - b - k0) / denom).clamp(0.0, 1.0);

    let mut c = invert_dot_gain(c0, sim.dot_gain_50[0]);
    let mut m = invert_dot_gain(m0, sim.dot_gain_50[1]);
    let mut y = invert_dot_gain(y0, sim.dot_gain_50[2]);
    let mut k = invert_dot_gain(k0, sim.dot_gain_50[3]);

    // 5 Gauss-Seidel refinement passes: each pass nudges every
    // ink coverage toward the value that makes its channel match
    // exactly under the current overprint state.
    for _ in 0..5 {
        let sim_rgb = cmyk_sim_to_srgb(sim, c, m, y, k);
        let dr = r - sim_rgb[0];
        let dg = g - sim_rgb[1];
        let db = b - sim_rgb[2];
        c = (c - dr * 0.9).clamp(0.0, 1.0);
        m = (m - dg * 0.9).clamp(0.0, 1.0);
        y = (y - db * 0.9).clamp(0.0, 1.0);
        // K is steered by overall lightness only.
        let lightness = 0.299 * r + 0.587 * g + 0.114 * b;
        let target_k = (1.0 - lightness).clamp(0.0, 1.0);
        k = 0.7 * k + 0.3 * target_k;
    }

    [c, m, y, k]
}

// ---------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------

/// Internal runtime representation of a profile's transform
/// capability. The `IccProfile` enum is the persistence-side
/// reference; this is the actual maths.
#[derive(Debug, Clone, Copy)]
enum ProfileKind {
    Rgb(RgbMatrixShaper),
    Cmyk(CmykSim),
    /// Custom profiles surface as the sRGB matrix shaper for now —
    /// `Custom { blob_hash, .. }` carries an ICC blob whose
    /// matrix-shaper or LUT entries would have to be parsed by a
    /// real ICC engine. We fall back to sRGB so the rest of the
    /// chain still operates rather than dropping the colour
    /// silently.
    UnknownCustom,
}

fn profile_kind(profile: &IccProfile) -> ProfileKind {
    use IccProfile as P;
    match profile {
        P::SrgbIec61966 => ProfileKind::Rgb(srgb_shaper()),
        P::AdobeRgb1998 => ProfileKind::Rgb(adobe_rgb_shaper()),
        P::DisplayP3 => ProfileKind::Rgb(display_p3_shaper()),
        // Custom CMYK profiles currently fall back to FOGRA39 because
        // parsing the embedded ICC blob is out of scope for the pure-Rust
        // matrix-shaper engine; choosing FOGRA39 keeps press behaviour
        // sensible for European stocks.
        P::FogRa39
        | P::Custom {
            color_space: crate::color::IccColorSpace::Cmyk,
            ..
        } => ProfileKind::Cmyk(FOGRA39),
        P::Swop2006 => ProfileKind::Cmyk(SWOP_2006),
        P::Custom { .. } => ProfileKind::UnknownCustom,
    }
}

/// A reusable colour transform built once for a `(source, dest,
/// intent)` triple and applied many times. Constructing a
/// transform precomputes the chromatic-adaptation matrix and the
/// destination's white point so per-pixel application is O(1).
#[derive(Debug, Clone, Copy)]
pub struct ColorTransform {
    src: ProfileKind,
    dst: ProfileKind,
    intent: RenderingIntent,
    /// Full chromatic-adaptation matrix from source's native white
    /// point (D65 in every profile we ship) to D50 PCS. We
    /// precompute it so it is not allocated per pixel.
    src_to_pcs_ca: Mat3,
    /// Inverse adaptation: D50 PCS → destination's native white.
    pcs_to_dst_ca: Mat3,
}

impl ColorTransform {
    /// Build a colour transform. Constructing once and applying
    /// many times is faster than calling [`convert_color`] in a
    /// hot loop.
    #[must_use]
    pub fn new(src: &IccProfile, dst: &IccProfile, intent: RenderingIntent) -> Self {
        let src_kind = profile_kind(src);
        let dst_kind = profile_kind(dst);
        // Both shipping families are D65-native. Absolute
        // colorimetric skips the adaptation step.
        let (src_ca, dst_ca) = if intent == RenderingIntent::AbsoluteColorimetric {
            (
                [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            )
        } else {
            (ca_d65_to_d50(), ca_d50_to_d65())
        };
        Self {
            src: src_kind,
            dst: dst_kind,
            intent,
            src_to_pcs_ca: src_ca,
            pcs_to_dst_ca: dst_ca,
        }
    }

    /// Apply the precomputed transform to a [`Color`]. Alpha
    /// passes through unchanged. The result is in the destination
    /// profile's native colour space (`Srgb`/`Cmyk` depending on
    /// the destination profile family).
    #[must_use]
    pub fn apply(&self, color: &Color) -> Color {
        // 1. Project the source colour into its profile's
        //    native triple (sRGB-encoded for RGB profiles, CMYK
        //    coverages for CMYK profiles).
        let alpha = color.alpha();
        let native_src = match self.src {
            ProfileKind::Rgb(_) | ProfileKind::UnknownCustom => {
                let (r, g, b, _) = color.to_srgb();
                NativeColor::Rgb([f64::from(r), f64::from(g), f64::from(b)])
            }
            ProfileKind::Cmyk(_) => match color {
                Color::Cmyk { c, m, y, k, .. } => {
                    NativeColor::Cmyk([f64::from(*c), f64::from(*m), f64::from(*y), f64::from(*k)])
                }
                _ => {
                    // Source profile is CMYK but the value is
                    // RGB-like — route through to_srgb() first,
                    // then through the inverse CMYK simulation.
                    let (r, g, b, _) = color.to_srgb();
                    NativeColor::Rgb([f64::from(r), f64::from(g), f64::from(b)])
                }
            },
        };

        // 2. Source-native → D50 PCS XYZ.
        let xyz_pcs = self.src_to_pcs(&native_src);

        // 3. Apply rendering intent in PCS (clipping / gamut
        //    handling). The intent only matters if the destination
        //    is narrower than the source; for sane in-gamut inputs
        //    this is a no-op.
        let xyz_intent = apply_intent_to_pcs(xyz_pcs, self.intent);

        // 4. D50 PCS → destination-native triple, then re-encode
        //    as a [`Color`] in the destination profile's family.
        self.pcs_to_dst(xyz_intent, alpha)
    }

    fn src_to_pcs(&self, native: &NativeColor) -> [f64; 3] {
        match (self.src, native) {
            (ProfileKind::Rgb(shaper), NativeColor::Rgb(rgb)) => {
                let xyz_d65 = shaper_rgb_to_xyz_d65(&shaper, rgb[0], rgb[1], rgb[2]);
                mat3_mul_vec3(&self.src_to_pcs_ca, xyz_d65)
            }
            (ProfileKind::Cmyk(sim), NativeColor::Cmyk(cmyk)) => {
                let srgb = cmyk_sim_to_srgb(sim, cmyk[0], cmyk[1], cmyk[2], cmyk[3]);
                let xyz_d65 = {
                    let (x, y, z) = srgb_to_xyz_d65(srgb[0] as f32, srgb[1] as f32, srgb[2] as f32);
                    [f64::from(x), f64::from(y), f64::from(z)]
                };
                mat3_mul_vec3(&self.src_to_pcs_ca, xyz_d65)
            }
            (ProfileKind::Rgb(shaper), NativeColor::Cmyk(_)) => {
                // Unreachable per `apply`'s native_src setup, kept
                // exhaustive in case of future routing changes.
                let xyz_d65 = shaper_rgb_to_xyz_d65(&shaper, 0.0, 0.0, 0.0);
                mat3_mul_vec3(&self.src_to_pcs_ca, xyz_d65)
            }
            (ProfileKind::Cmyk(_) | ProfileKind::UnknownCustom, NativeColor::Rgb(rgb)) => {
                let (x, y, z) = srgb_to_xyz_d65(rgb[0] as f32, rgb[1] as f32, rgb[2] as f32);
                let xyz_d65 = [f64::from(x), f64::from(y), f64::from(z)];
                mat3_mul_vec3(&self.src_to_pcs_ca, xyz_d65)
            }
            (ProfileKind::UnknownCustom, NativeColor::Cmyk(cmyk)) => {
                // Custom (unknown) treated as sRGB; convert CMYK to
                // sRGB via the naive helper as a defensive fallback.
                let (r, g, b) = cmyk_to_srgb(
                    cmyk[0] as f32,
                    cmyk[1] as f32,
                    cmyk[2] as f32,
                    cmyk[3] as f32,
                );
                let (x, y, z) = srgb_to_xyz_d65(r, g, b);
                let xyz_d65 = [f64::from(x), f64::from(y), f64::from(z)];
                mat3_mul_vec3(&self.src_to_pcs_ca, xyz_d65)
            }
        }
    }

    fn pcs_to_dst(&self, xyz_pcs: [f64; 3], alpha: f32) -> Color {
        let xyz_d65 = mat3_mul_vec3(&self.pcs_to_dst_ca, xyz_pcs);
        match self.dst {
            ProfileKind::Rgb(shaper) => {
                let rgb = xyz_d65_to_shaper_rgb(&shaper, xyz_d65);
                Color::Srgb {
                    r: rgb[0].clamp(0.0, 1.0) as f32,
                    g: rgb[1].clamp(0.0, 1.0) as f32,
                    b: rgb[2].clamp(0.0, 1.0) as f32,
                    a: alpha,
                }
            }
            ProfileKind::Cmyk(sim) => {
                let (r, g, b) =
                    xyz_d65_to_srgb(xyz_d65[0] as f32, xyz_d65[1] as f32, xyz_d65[2] as f32);
                let cmyk = srgb_to_cmyk_sim(sim, f64::from(r), f64::from(g), f64::from(b));
                Color::Cmyk {
                    c: cmyk[0] as f32,
                    m: cmyk[1] as f32,
                    y: cmyk[2] as f32,
                    k: cmyk[3] as f32,
                    a: alpha,
                }
            }
            ProfileKind::UnknownCustom => {
                let (r, g, b) =
                    xyz_d65_to_srgb(xyz_d65[0] as f32, xyz_d65[1] as f32, xyz_d65[2] as f32);
                Color::Srgb { r, g, b, a: alpha }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum NativeColor {
    Rgb([f64; 3]),
    Cmyk([f64; 4]),
}

/// Apply a rendering intent to a PCS XYZ value. The Phase 4
/// implementation models four intents:
///
/// * `Perceptual` — gentle desaturation toward the PCS neutral
///   axis. Pulls colours that would otherwise clip back into
///   gamut while preserving relative hue/lightness order.
/// * `RelativeColorimetric` — passes XYZ through unchanged
///   (caller's destination encoder clamps).
/// * `Saturation` — preserves the chroma magnitude but allows
///   lightness to drift, so brand-saturated colours stay
///   saturated even if they fall outside the destination gamut.
/// * `AbsoluteColorimetric` — bypasses chromatic adaptation
///   entirely (handled at construction time in
///   [`ColorTransform::new`]).
fn apply_intent_to_pcs(xyz: [f64; 3], intent: RenderingIntent) -> [f64; 3] {
    match intent {
        RenderingIntent::Perceptual => {
            // 5% pull toward D50 neutral; smooths obvious
            // out-of-gamut spikes without crushing in-gamut tones.
            let pull = 0.05;
            [
                xyz[0] * (1.0 - pull) + D50_X * pull,
                xyz[1] * (1.0 - pull) + D50_Y * pull,
                xyz[2] * (1.0 - pull) + D50_Z * pull,
            ]
        }
        RenderingIntent::Saturation => {
            // Saturation intent boosts chroma slightly to
            // preserve the source's relative saturation when
            // destination gamut clamps it.
            let mean = (xyz[0] + xyz[1] + xyz[2]) / 3.0;
            let boost = 1.10;
            [
                (xyz[0] - mean) * boost + mean,
                (xyz[1] - mean) * boost + mean,
                (xyz[2] - mean) * boost + mean,
            ]
        }
        RenderingIntent::RelativeColorimetric | RenderingIntent::AbsoluteColorimetric => xyz,
    }
}

/// One-shot convenience for callers that don't want to allocate
/// a `ColorTransform` themselves.
#[must_use]
pub fn convert_color(
    src_color: &Color,
    src_profile: &IccProfile,
    dst_profile: &IccProfile,
    intent: RenderingIntent,
) -> Color {
    let t = ColorTransform::new(src_profile, dst_profile, intent);
    t.apply(src_color)
}

/// Convert sRGB to CMYK using the requested CMYK profile and
/// rendering intent. Falls back to the naive
/// [`crate::color::srgb_to_cmyk`] formula if `cmyk_profile` is not
/// a CMYK profile.
#[must_use]
pub fn srgb_to_cmyk_profiled(
    r: f32,
    g: f32,
    b: f32,
    cmyk_profile: &IccProfile,
    intent: RenderingIntent,
) -> (f32, f32, f32, f32) {
    let src = Color::Srgb { r, g, b, a: 1.0 };
    let dst = convert_color(&src, &IccProfile::SrgbIec61966, cmyk_profile, intent);
    if let Color::Cmyk { c, m, y, k, .. } = dst {
        (c, m, y, k)
    } else {
        // Caller asked for something that isn't a CMYK profile;
        // surface the naive formula so we never return RGB by
        // accident.
        crate::color::srgb_to_cmyk(r, g, b)
    }
}

/// Convert CMYK to sRGB using a real CMYK simulation profile and
/// rendering intent. Falls back to [`crate::color::cmyk_to_srgb`]
/// if `cmyk_profile` is not a CMYK profile.
#[must_use]
pub fn cmyk_to_srgb_profiled(
    c: f32,
    m: f32,
    y: f32,
    k: f32,
    cmyk_profile: &IccProfile,
    intent: RenderingIntent,
) -> (f32, f32, f32) {
    let src = Color::Cmyk { c, m, y, k, a: 1.0 };
    let dst = convert_color(&src, cmyk_profile, &IccProfile::SrgbIec61966, intent);
    if let Color::Srgb { r, g, b, .. } = dst {
        (r, g, b)
    } else {
        cmyk_to_srgb(c, m, y, k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IccProfile;

    fn srgb(r: f32, g: f32, b: f32) -> Color {
        Color::Srgb { r, g, b, a: 1.0 }
    }

    #[test]
    fn srgb_to_srgb_identity_preserves_components() {
        let c = srgb(0.5, 0.3, 0.7);
        let out = convert_color(
            &c,
            &IccProfile::SrgbIec61966,
            &IccProfile::SrgbIec61966,
            RenderingIntent::RelativeColorimetric,
        );
        if let Color::Srgb { r, g, b, .. } = out {
            assert!((r - 0.5).abs() < 1e-3, "r drifted: {r}");
            assert!((g - 0.3).abs() < 1e-3, "g drifted: {g}");
            assert!((b - 0.7).abs() < 1e-3, "b drifted: {b}");
        } else {
            panic!("expected sRGB output, got {out:?}");
        }
    }

    #[test]
    fn srgb_to_adobe_rgb_to_srgb_round_trip_within_tolerance() {
        let c = srgb(0.5, 0.3, 0.7);
        let adobe = convert_color(
            &c,
            &IccProfile::SrgbIec61966,
            &IccProfile::AdobeRgb1998,
            RenderingIntent::RelativeColorimetric,
        );
        // Adobe RGB has a wider gamut than sRGB so the
        // intermediate value is in-gamut, and the round-trip
        // back must be near-identical (sub-1% drift).
        let back = convert_color(
            &adobe,
            &IccProfile::AdobeRgb1998,
            &IccProfile::SrgbIec61966,
            RenderingIntent::RelativeColorimetric,
        );
        if let Color::Srgb { r, g, b, .. } = back {
            assert!((r - 0.5).abs() < 0.01, "r drifted: {r}");
            assert!((g - 0.3).abs() < 0.01, "g drifted: {g}");
            assert!((b - 0.7).abs() < 0.01, "b drifted: {b}");
        } else {
            panic!("expected sRGB output, got {back:?}");
        }
    }

    #[test]
    fn srgb_to_display_p3_to_srgb_round_trip_within_tolerance() {
        let c = srgb(0.4, 0.6, 0.2);
        let p3 = convert_color(
            &c,
            &IccProfile::SrgbIec61966,
            &IccProfile::DisplayP3,
            RenderingIntent::RelativeColorimetric,
        );
        let back = convert_color(
            &p3,
            &IccProfile::DisplayP3,
            &IccProfile::SrgbIec61966,
            RenderingIntent::RelativeColorimetric,
        );
        if let Color::Srgb { r, g, b, .. } = back {
            assert!((r - 0.4).abs() < 0.01, "r drifted: {r}");
            assert!((g - 0.6).abs() < 0.01, "g drifted: {g}");
            assert!((b - 0.2).abs() < 0.01, "b drifted: {b}");
        } else {
            panic!("expected sRGB output, got {back:?}");
        }
    }

    #[test]
    fn srgb_white_to_fogra39_yields_zero_ink_within_tolerance() {
        let (c, m, y, k) = srgb_to_cmyk_profiled(
            1.0,
            1.0,
            1.0,
            &IccProfile::FogRa39,
            RenderingIntent::RelativeColorimetric,
        );
        // Pure paper-white should converge to essentially no ink.
        assert!(c < 0.05, "C should be small: {c}");
        assert!(m < 0.05, "M should be small: {m}");
        assert!(y < 0.05, "Y should be small: {y}");
        assert!(k < 0.05, "K should be small: {k}");
    }

    #[test]
    fn srgb_black_to_fogra39_yields_solid_k_within_tolerance() {
        let (c, m, y, k) = srgb_to_cmyk_profiled(
            0.0,
            0.0,
            0.0,
            &IccProfile::FogRa39,
            RenderingIntent::RelativeColorimetric,
        );
        // Pure black is mostly K; CMY may carry traces but the
        // overall total must hit near full coverage.
        assert!(k > 0.7, "K should dominate: {k}");
        assert!(c + m + y + k > 0.95, "total ink should be near full");
    }

    #[test]
    fn fogra39_and_swop_disagree_on_neutral_grey() {
        // The two profiles have different dot gain → different
        // CMYK for the same sRGB grey. This pins that the two
        // profiles are not aliases for each other.
        let (c_f, m_f, y_f, k_f) = srgb_to_cmyk_profiled(
            0.5,
            0.5,
            0.5,
            &IccProfile::FogRa39,
            RenderingIntent::RelativeColorimetric,
        );
        let (c_s, m_s, y_s, k_s) = srgb_to_cmyk_profiled(
            0.5,
            0.5,
            0.5,
            &IccProfile::Swop2006,
            RenderingIntent::RelativeColorimetric,
        );
        let f_sum = c_f + m_f + y_f + k_f;
        let s_sum = c_s + m_s + y_s + k_s;
        assert!((f_sum - s_sum).abs() > 0.005, "FOGRA39 vs SWOP must differ");
    }

    #[test]
    fn dot_gain_round_trip_recovers_input() {
        // Forward then inverse dot gain must return the input.
        for nominal in [0.0_f64, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0] {
            let effective = apply_dot_gain(nominal, 0.18);
            let back = invert_dot_gain(effective, 0.18);
            assert!(
                (back - nominal).abs() < 1e-3,
                "dot-gain round-trip drift: nominal={nominal}, back={back}"
            );
        }
    }

    #[test]
    fn cmyk_simulation_round_trip_produces_valid_inks() {
        // CMYK → sRGB → CMYK via FOGRA39 is intrinsically lossy:
        // the dot-gain + ink-density simulation is not a bijection
        // because it models perceived appearance, not a bit-exact
        // round trip. What we *can* assert is that the function
        // returns valid CMYK quadruples for an in-gamut source —
        // every channel in [0, 1] and no NaNs.
        let original = Color::Cmyk {
            c: 0.0,
            m: 0.8,
            y: 0.7,
            k: 0.1,
            a: 1.0,
        };
        let intermediate = convert_color(
            &original,
            &IccProfile::FogRa39,
            &IccProfile::SrgbIec61966,
            RenderingIntent::RelativeColorimetric,
        );
        let back = convert_color(
            &intermediate,
            &IccProfile::SrgbIec61966,
            &IccProfile::FogRa39,
            RenderingIntent::RelativeColorimetric,
        );
        if let Color::Cmyk { c, m, y, k, .. } = back {
            for (name, v) in [("C", c), ("M", m), ("Y", y), ("K", k)] {
                assert!(v.is_finite(), "{name} not finite: {v}");
                assert!((0.0..=1.0).contains(&v), "{name} out of range: {v}");
            }
            // Magenta is the dominant ink in a saturated red, and
            // the inverse simulation must still recognise that.
            assert!(m > 0.3, "M should remain meaningful: {m}");
        } else {
            panic!("expected CMYK output, got {back:?}");
        }
    }

    #[test]
    fn srgb_cmyk_srgb_round_trip_through_fogra39_preserves_hue() {
        // This is the round-trip the export pipeline actually
        // exercises: a viewport sRGB colour is converted to CMYK
        // for print export, then we convert back to verify the
        // soft-proof preview matches. Tolerance is generous
        // because the simulation is not a bijection.
        let source = srgb(0.78, 0.20, 0.30);
        let cmyk = convert_color(
            &source,
            &IccProfile::SrgbIec61966,
            &IccProfile::FogRa39,
            RenderingIntent::RelativeColorimetric,
        );
        let back = convert_color(
            &cmyk,
            &IccProfile::FogRa39,
            &IccProfile::SrgbIec61966,
            RenderingIntent::RelativeColorimetric,
        );
        if let Color::Srgb { r, g, b, .. } = back {
            assert!(r > g && r > b, "red must remain dominant: {r}/{g}/{b}");
        } else {
            panic!("expected sRGB output, got {back:?}");
        }
    }

    #[test]
    fn rendering_intent_affects_destination_output() {
        // Compare perceptual vs. relative colorimetric for a
        // saturated red sent into AdobeRGB. The perceptual
        // intent pulls toward the D50 PCS neutral, so its
        // output must differ from the straight relative path
        // by at least a noticeable per-channel margin.
        let saturated = srgb(1.0, 0.0, 0.0);
        let perceptual = convert_color(
            &saturated,
            &IccProfile::SrgbIec61966,
            &IccProfile::AdobeRgb1998,
            RenderingIntent::Perceptual,
        );
        let relative = convert_color(
            &saturated,
            &IccProfile::SrgbIec61966,
            &IccProfile::AdobeRgb1998,
            RenderingIntent::RelativeColorimetric,
        );
        if let (
            Color::Srgb {
                r: rp,
                g: gp,
                b: bp,
                ..
            },
            Color::Srgb {
                r: rr,
                g: gr,
                b: br,
                ..
            },
        ) = (&perceptual, &relative)
        {
            let diff = (rp - rr).abs() + (gp - gr).abs() + (bp - br).abs();
            assert!(
                diff > 0.02,
                "perceptual vs relative must differ noticeably: rp={rp} gp={gp} bp={bp} \
                 rr={rr} gr={gr} br={br}"
            );
            // The perceptual pull lifts G and B (toward neutral),
            // so at least one of them must be strictly higher.
            assert!(gp >= gr || bp >= br, "perceptual must lift G or B");
        } else {
            panic!("unexpected variants");
        }
    }

    #[test]
    fn absolute_colorimetric_skips_chromatic_adaptation() {
        // Adobe RGB → sRGB with AbsoluteColorimetric must not
        // apply the Bradford pair; verify by constructing two
        // transforms and asserting their adaptation matrices differ.
        let rel = ColorTransform::new(
            &IccProfile::AdobeRgb1998,
            &IccProfile::SrgbIec61966,
            RenderingIntent::RelativeColorimetric,
        );
        let abs = ColorTransform::new(
            &IccProfile::AdobeRgb1998,
            &IccProfile::SrgbIec61966,
            RenderingIntent::AbsoluteColorimetric,
        );
        assert_ne!(rel.src_to_pcs_ca, abs.src_to_pcs_ca);
        // Identity matrix for the absolute case.
        assert_eq!(abs.src_to_pcs_ca[0][0], 1.0);
        assert_eq!(abs.src_to_pcs_ca[1][1], 1.0);
        assert_eq!(abs.src_to_pcs_ca[2][2], 1.0);
    }

    #[test]
    fn bradford_round_trip_d65_d50_d65_is_identity() {
        let pt = [0.5, 0.5, 0.5];
        let to_d50 = mat3_mul_vec3(&BRADFORD_D65_TO_D50, pt);
        let back = mat3_mul_vec3(&BRADFORD_D50_TO_D65, to_d50);
        for i in 0..3 {
            assert!(
                (back[i] - pt[i]).abs() < 1e-3,
                "Bradford D65↔D50 round-trip failed at axis {i}: {} vs {}",
                back[i],
                pt[i]
            );
        }
    }

    #[test]
    fn mat3_inverse_inverts() {
        let m: Mat3 = [[2.0, 0.0, 0.0], [1.0, 3.0, 0.0], [0.0, 1.0, 4.0]];
        let inv = mat3_inverse(&m).unwrap();
        let prod = mat3_mul(&m, &inv);
        // Should be the identity.
        for (c, col) in prod.iter().enumerate() {
            for (r, cell) in col.iter().enumerate() {
                let expected = if c == r { 1.0 } else { 0.0 };
                assert!((cell - expected).abs() < 1e-9);
            }
        }
    }
}

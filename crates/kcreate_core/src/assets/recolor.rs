//! Theme/brand-aware recolour for inserted assets (workstream H3).
//!
//! When an asset is dropped onto the canvas, KCreate nudges its authored
//! colours toward the document's active theme/brand **accent** so inserted
//! art matches the deck — while keeping every node a fully editable
//! `VectorLayer`. The maths here is pure ([`RgbaColor`] in, [`RgbaColor`]
//! out): the bridge reads the accent from the live document and applies a
//! [`Recolor`] to each placed path's fill/stroke.
//!
//! Two regimes keep the result legible:
//!
//! * **Mono** art (a single authored colour — an outline icon stroked in
//!   near-black, or a single-fill shape) is painted the accent outright,
//!   preserving alpha. This is the common case and gives the strongest
//!   "on brand" signal.
//! * **Multi**-colour art (flat illustrations) keeps its composition: only
//!   *chromatic* colours (chroma >= [`NEUTRAL_CHROMA`]) are rotated to the
//!   accent's hue, keeping each colour's own lightness (so shading and depth
//!   survive) and blending saturation toward the accent. Neutrals — white,
//!   black, grey — are left untouched so highlights and outlines stay crisp.
//!
//! With no theme set the bridge skips recolour entirely (a neutral default),
//! so a brand-new document inserts assets in their authored colours.

use crate::color::{hsl_to_srgb, srgb_to_hsl};
use crate::node::RgbaColor;
use crate::theme::{chroma, NEUTRAL_CHROMA};

/// A recolour plan toward a single `accent`, built once per insert from the
/// asset's distinct authored colours and then applied to every fill/stroke.
#[derive(Debug, Clone, Copy)]
pub struct Recolor {
    accent: RgbaColor,
    accent_hue: f32,
    accent_sat: f32,
    /// Whether the source art is a single colour (→ paint it the accent) or
    /// multi-colour (→ hue-rotate chromatics only).
    mono: bool,
}

impl Recolor {
    /// Build a plan for art whose distinct authored colours are `palette`,
    /// targeting `accent`. `palette` should contain every fill/stroke colour
    /// the asset paints (order irrelevant; duplicates are fine).
    #[must_use]
    pub fn plan(palette: &[RgbaColor], accent: RgbaColor) -> Self {
        let (accent_hue, accent_sat, _) = srgb_to_hsl(accent.r, accent.g, accent.b);
        Self {
            accent,
            accent_hue,
            accent_sat,
            mono: distinct_visible_colors(palette) <= 1,
        }
    }

    /// Map one authored colour to its recoloured value.
    #[must_use]
    pub fn apply(&self, c: RgbaColor) -> RgbaColor {
        // Fully transparent paint carries no colour — leave it alone.
        if c.a <= 0.0 {
            return c;
        }
        if self.mono {
            // Single-colour art: adopt the accent, keep the source alpha.
            return RgbaColor {
                a: c.a,
                ..self.accent
            };
        }
        if chroma(c) < NEUTRAL_CHROMA {
            // Neutral (white/black/grey): keep it so outlines and highlights
            // stay crisp against the recoloured chromatics.
            return c;
        }
        // Chromatic: take the accent's hue but keep this colour's own
        // lightness, and meet the accent halfway on saturation so the whole
        // illustration reads as one brand family without going flat.
        let (_, s, l) = srgb_to_hsl(c.r, c.g, c.b);
        let s = f32::midpoint(s, self.accent_sat);
        let (r, g, b) = hsl_to_srgb(self.accent_hue, s, l);
        RgbaColor { r, g, b, a: c.a }
    }
}

/// Count distinct *visible* colours (alpha > 0), quantised to 8-bit RGB so
/// floating-point dust doesn't read as extra colours.
fn distinct_visible_colors(palette: &[RgbaColor]) -> usize {
    let mut seen: Vec<[u8; 3]> = Vec::new();
    for c in palette {
        if c.a <= 0.0 {
            continue;
        }
        let key = [quant(c.r), quant(c.g), quant(c.b)];
        if !seen.contains(&key) {
            seen.push(key);
        }
    }
    seen.len()
}

/// Quantise a `[0, 1]` channel to 8-bit.
fn quant(x: f32) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    fn hue(c: RgbaColor) -> f32 {
        srgb_to_hsl(c.r, c.g, c.b).0
    }

    fn light(c: RgbaColor) -> f32 {
        srgb_to_hsl(c.r, c.g, c.b).2
    }

    #[test]
    fn mono_art_is_painted_the_accent_keeping_alpha() {
        let accent = RgbaColor::new(0.9, 0.1, 0.1, 1.0);
        // A single near-black stroke at half opacity (a typical outline icon).
        let src = RgbaColor::new(0.12, 0.16, 0.22, 0.5);
        let plan = Recolor::plan(&[src], accent);
        let out = plan.apply(src);
        assert!(
            approx(out.r, accent.r) && approx(out.g, accent.g) && approx(out.b, accent.b),
            "mono art should adopt the accent rgb, got {out:?}"
        );
        assert!(approx(out.a, 0.5), "source alpha must be preserved");
    }

    #[test]
    fn multi_art_rotates_chromatics_to_accent_hue() {
        let accent = RgbaColor::new(0.85, 0.15, 0.15, 1.0); // red, hue ~0
        let accent_hue = hue(accent);
        let blue = RgbaColor::new(0.2, 0.4, 0.9, 1.0);
        let white = RgbaColor::new(1.0, 1.0, 1.0, 1.0);
        let black = RgbaColor::new(0.05, 0.05, 0.05, 1.0);
        let plan = Recolor::plan(&[blue, white, black], accent);

        let rb = plan.apply(blue);
        let dh = (hue(rb) - accent_hue).abs();
        assert!(
            !(1.0..=359.0).contains(&dh),
            "chromatic hue should match the accent, got {}",
            hue(rb)
        );
        assert!(approx(rb.a, 1.0), "alpha preserved");

        // Neutrals survive untouched.
        assert_eq!(plan.apply(white), white, "white must be left alone");
        assert_eq!(plan.apply(black), black, "black must be left alone");
    }

    #[test]
    fn multi_art_preserves_relative_lightness() {
        // Shading must survive: a lighter source stays lighter than a darker
        // one after recolour.
        let accent = RgbaColor::new(0.1, 0.3, 0.9, 1.0);
        let lighter = RgbaColor::new(0.95, 0.55, 0.55, 1.0);
        let darker = RgbaColor::new(0.45, 0.1, 0.1, 1.0);
        let plan = Recolor::plan(&[lighter, darker], accent);
        assert!(
            light(plan.apply(lighter)) > light(plan.apply(darker)),
            "relative lightness ordering must be preserved"
        );
    }

    #[test]
    fn transparent_paint_is_left_alone() {
        let accent = RgbaColor::new(0.9, 0.1, 0.1, 1.0);
        let clear = RgbaColor::new(0.0, 0.0, 0.0, 0.0);
        let blue = RgbaColor::new(0.2, 0.4, 0.9, 1.0);
        let plan = Recolor::plan(&[clear, blue], accent);
        assert_eq!(plan.apply(clear), clear);
        // The clear colour does not count toward distinctness, so a single
        // visible colour alongside it still reads as mono.
        assert!(plan.mono, "one visible colour + transparent ⇒ mono");
    }

    #[test]
    fn distinct_count_ignores_transparent_and_dedupes() {
        let a = RgbaColor::new(0.2, 0.4, 0.9, 1.0);
        let a_dup = RgbaColor::new(0.2001, 0.4001, 0.9001, 1.0);
        let clear = RgbaColor::new(1.0, 0.0, 0.0, 0.0);
        assert_eq!(distinct_visible_colors(&[a, a_dup, clear]), 1);
    }
}

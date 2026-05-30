//! Palette harmonisation — Phase 10 Block B Task 11.
//!
//! Given an input palette, the harmoniser nudges hues so the result
//! aligns with a colour-theory harmony rule:
//!
//! - **Complementary** — two hues 180° apart.
//! - **Triadic** — three hues 120° apart.
//! - **Analogous** — neighbours within ±30°.
//! - **Split-complementary** — base + two hues flanking the complement.
//! - **Tetradic** — four hues forming a rectangle (two complementary
//!   pairs offset by 60°).
//! - **Auto** — pick the rule whose required hue layout is closest
//!   to the input.
//!
//! The algorithm:
//!
//! 1. Convert every input colour to HSL.
//! 2. Anchor the harmony to the first colour's hue (`base_hue`).
//! 3. Compute the set of "target hues" required by the rule.
//! 4. For every colour after the first, snap to the nearest target
//!    hue and keep its saturation / luminance.
//!
//! All colour arithmetic is finite + deterministic — pure
//! functions, no randomness, no network, no model dependency.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HarmonyRule {
    Auto,
    Complementary,
    Triadic,
    Analogous,
    SplitComplementary,
    Tetradic,
}

impl HarmonyRule {
    /// Map a wire string into a rule. Accepts both the snake_case
    /// serde rendering and the camelCase form used in the renderer.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(Self::Auto),
            "complementary" => Some(Self::Complementary),
            "triadic" => Some(Self::Triadic),
            "analogous" => Some(Self::Analogous),
            "split_complementary" | "splitComplementary" => Some(Self::SplitComplementary),
            "tetradic" => Some(Self::Tetradic),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarmonySuggestion {
    pub input_hex: String,
    pub suggested_hex: String,
    pub hue_shift_degrees: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarmonyResult {
    pub rule: HarmonyRule,
    pub suggestions: Vec<HarmonySuggestion>,
}

#[derive(Debug, Error)]
pub enum HarmonyError {
    #[error("harmonize: empty palette")]
    Empty,
    #[error("harmonize: invalid hex colour '{0}'")]
    BadHex(String),
}

/// Harmonise `palette_hex` under `rule`. Returns one suggestion per
/// input colour; the first colour is always anchored (zero shift).
///
/// # Errors
///
/// Returns [`HarmonyError::Empty`] when the palette is empty or
/// [`HarmonyError::BadHex`] when any input colour fails to parse.
pub fn harmonize_palette(
    palette_hex: &[String],
    rule: HarmonyRule,
) -> Result<HarmonyResult, HarmonyError> {
    if palette_hex.is_empty() {
        return Err(HarmonyError::Empty);
    }
    let hsl: Vec<Hsl> = palette_hex
        .iter()
        .map(|h| parse_hex(h).ok_or_else(|| HarmonyError::BadHex(h.clone())))
        .collect::<Result<_, _>>()?;

    let chosen_rule = match rule {
        HarmonyRule::Auto => choose_rule_auto(&hsl),
        other => other,
    };
    let targets = rule_targets(chosen_rule, hsl[0].h);
    let mut out: Vec<HarmonySuggestion> = Vec::with_capacity(hsl.len());
    for (i, c) in hsl.iter().enumerate() {
        let target_h = if i == 0 {
            c.h
        } else {
            nearest_target(c.h, &targets)
        };
        let snapped = Hsl {
            h: target_h,
            s: c.s,
            l: c.l,
            a: c.a,
        };
        let shift = wrap_degrees(target_h - c.h);
        out.push(HarmonySuggestion {
            input_hex: palette_hex[i].clone(),
            suggested_hex: snapped.to_hex(),
            hue_shift_degrees: shift,
        });
    }
    Ok(HarmonyResult {
        rule: chosen_rule,
        suggestions: out,
    })
}

/// Choose the rule whose required hue layout matches the input the
/// best. We pick the rule with the smallest total "distance from
/// nearest target" sum.
fn choose_rule_auto(hsl: &[Hsl]) -> HarmonyRule {
    use HarmonyRule::{Analogous, Complementary, SplitComplementary, Tetradic, Triadic};
    let base = hsl[0].h;
    let candidates = [
        Complementary,
        Triadic,
        Analogous,
        SplitComplementary,
        Tetradic,
    ];
    let mut best = Complementary;
    let mut best_cost = f32::INFINITY;
    for rule in candidates {
        let targets = rule_targets(rule, base);
        // Use `wrap_degrees` so circular distances are measured
        // correctly (e.g. hue 350° → target 10° is 20°, not 340°).
        let cost: f32 = hsl
            .iter()
            .skip(1)
            .map(|c| wrap_degrees(c.h - nearest_target(c.h, &targets)).abs())
            .sum();
        if cost < best_cost {
            best_cost = cost;
            best = rule;
        }
    }
    best
}

fn rule_targets(rule: HarmonyRule, base: f32) -> Vec<f32> {
    let wrap = |h: f32| ((h % 360.0) + 360.0) % 360.0;
    match rule {
        HarmonyRule::Auto => vec![base], // unreachable in caller
        HarmonyRule::Complementary => vec![wrap(base), wrap(base + 180.0)],
        HarmonyRule::Triadic => vec![wrap(base), wrap(base + 120.0), wrap(base + 240.0)],
        HarmonyRule::Analogous => vec![wrap(base - 30.0), wrap(base), wrap(base + 30.0)],
        HarmonyRule::SplitComplementary => vec![wrap(base), wrap(base + 150.0), wrap(base + 210.0)],
        HarmonyRule::Tetradic => vec![
            wrap(base),
            wrap(base + 60.0),
            wrap(base + 180.0),
            wrap(base + 240.0),
        ],
    }
}

fn nearest_target(h: f32, targets: &[f32]) -> f32 {
    let mut best = targets[0];
    let mut best_dist = wrap_degrees(h - best).abs();
    for &t in targets.iter().skip(1) {
        let d = wrap_degrees(h - t).abs();
        if d < best_dist {
            best_dist = d;
            best = t;
        }
    }
    best
}

fn wrap_degrees(d: f32) -> f32 {
    let mut x = d % 360.0;
    if x > 180.0 {
        x -= 360.0;
    } else if x < -180.0 {
        x += 360.0;
    }
    x
}

#[derive(Debug, Clone, Copy)]
struct Hsl {
    h: f32,
    s: f32,
    l: f32,
    a: f32,
}

impl Hsl {
    fn to_hex(self) -> String {
        let [r, g, b] = hsl_to_rgb(self.h, self.s, self.l);
        let a = (self.a.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

fn parse_hex(s: &str) -> Option<Hsl> {
    let s = s.trim().trim_start_matches('#');
    let (r, g, b, a) = match s.len() {
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            (r, g, b, 255u8)
        }
        8 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            let a = u8::from_str_radix(&s[6..8], 16).ok()?;
            (r, g, b, a)
        }
        _ => return None,
    };
    let (h, s, l) = rgb_to_hsl(r, g, b);
    Some(Hsl {
        h,
        s,
        l,
        a: f32::from(a) / 255.0,
    })
}

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let r = f32::from(r) / 255.0;
    let g = f32::from(g) / 255.0;
    let b = f32::from(b) / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = f32::midpoint(max, min);
    if (max - min).abs() < f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if (max - r).abs() < f32::EPSILON {
        ((g - b) / d) + if g < b { 6.0 } else { 0.0 }
    } else if (max - g).abs() < f32::EPSILON {
        ((b - r) / d) + 2.0
    } else {
        ((r - g) / d) + 4.0
    };
    (h * 60.0, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [u8; 3] {
    if s <= 0.0 {
        let v = (l * 255.0).round().clamp(0.0, 255.0) as u8;
        return [v, v, v];
    }
    let h = ((h % 360.0) + 360.0) % 360.0 / 360.0;
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let to_v = |t: f32| {
        let mut t = t;
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        let r = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 1.0 / 2.0 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (r * 255.0).round().clamp(0.0, 255.0) as u8
    };
    [to_v(h + 1.0 / 3.0), to_v(h), to_v(h - 1.0 / 3.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_palette_errors() {
        let err = harmonize_palette(&[], HarmonyRule::Triadic).unwrap_err();
        assert!(matches!(err, HarmonyError::Empty));
    }

    #[test]
    fn bad_hex_errors() {
        let err = harmonize_palette(&["not_a_hex".into()], HarmonyRule::Triadic).unwrap_err();
        assert!(matches!(err, HarmonyError::BadHex(_)));
    }

    #[test]
    fn anchor_colour_is_preserved() {
        let r = harmonize_palette(
            &["#ff0000ff".into(), "#00ff00ff".into()],
            HarmonyRule::Complementary,
        )
        .unwrap();
        assert_eq!(r.suggestions[0].suggested_hex, "#ff0000ff");
        assert!((r.suggestions[0].hue_shift_degrees).abs() < 1e-3);
    }

    #[test]
    fn complementary_pulls_to_opposite() {
        let r = harmonize_palette(
            &["#ff0000ff".into(), "#00ff00ff".into()],
            HarmonyRule::Complementary,
        )
        .unwrap();
        // Red at hue 0; complement at hue 180 (cyan). Green at hue 120
        // should snap to one of those — the closer one is 180 (cyan).
        // Verify the suggested hex is not still pure green.
        assert_ne!(r.suggestions[1].suggested_hex, "#00ff00ff");
    }

    #[test]
    fn analogous_keeps_hues_close_to_base() {
        let palette: Vec<String> = vec![
            "#ff0000ff".into(), // 0°
            "#ffaa00ff".into(), // ~40°
            "#00ff00ff".into(), // 120°
        ];
        let r = harmonize_palette(&palette, HarmonyRule::Analogous).unwrap();
        // Each suggestion's shift should pull hues into ±30° of red.
        for s in &r.suggestions {
            assert!(
                s.hue_shift_degrees.abs() <= 90.0,
                "shift {} too large for analogous rule",
                s.hue_shift_degrees
            );
        }
    }

    #[test]
    fn auto_returns_some_concrete_rule() {
        let palette: Vec<String> = vec!["#ff0000ff".into(), "#00ffffff".into()];
        let r = harmonize_palette(&palette, HarmonyRule::Auto).unwrap();
        assert_ne!(r.rule, HarmonyRule::Auto);
    }

    #[test]
    fn rule_from_wire_accepts_canonical_names() {
        assert_eq!(
            HarmonyRule::from_wire("split_complementary"),
            Some(HarmonyRule::SplitComplementary)
        );
        assert_eq!(
            HarmonyRule::from_wire("splitComplementary"),
            Some(HarmonyRule::SplitComplementary)
        );
        assert_eq!(HarmonyRule::from_wire("foo"), None);
    }

    #[test]
    fn rgb_hsl_round_trip_is_stable() {
        for &c in &[[255u8, 0, 0], [0, 255, 0], [0, 0, 255], [128, 64, 32]] {
            let (h, s, l) = rgb_to_hsl(c[0], c[1], c[2]);
            let back = hsl_to_rgb(h, s, l);
            // Allow ±1 quantisation drift.
            for i in 0..3 {
                assert!(
                    (i32::from(back[i]) - i32::from(c[i])).abs() <= 1,
                    "rgb→hsl→rgb drift at channel {i}"
                );
            }
        }
    }
}

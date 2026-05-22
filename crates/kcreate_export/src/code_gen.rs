//! Inspect-mode code generation.
//!
//! Three pure functions [`node_to_css`], [`node_to_tailwind`], and
//! [`node_to_react_style`] turn a [`Node`] into a string the user
//! can copy from the Inspect panel and paste into a real codebase.
//!
//! The mapping is intentionally lossy — these emit a *handoff
//! template*, not a pixel-perfect reproduction. Effects like
//! gradients on text or compound box-shadows fall back to a
//! best-effort approximation, and any node-level metadata that
//! doesn't have a clean CSS analogue is dropped. The goal is "the
//! developer copies this snippet, tweaks one or two values, and
//! ships" — not full CSS preservation.
//!
//! All three emitters are deterministic and side-effect-free.
//! Floating-point values are rendered with [`fmt_num`] which trims
//! trailing zeros so `12.0` becomes `12px` and `1.5` becomes
//! `1.5px`.

use std::fmt::Write as _;

use kcreate_core::{
    Bounds, Constraint, Effect, FillStyle, GradientKind, Node, NodeType, RgbaColor, StrokeStyle,
};

/// Emit a CSS rule body (no selector, no braces) describing the
/// node's painted appearance. Pair with a class or id selector at
/// the call site:
///
/// ```text
/// .my-button {
///     /* node_to_css(...) goes here */
/// }
/// ```
#[must_use]
pub fn node_to_css(node: &Node) -> String {
    let mut out = String::new();
    push_position(&mut out, node);
    push_size(&mut out, node);
    push_opacity(&mut out, node);
    push_fill_css(&mut out, &node.style.fill);
    push_stroke_css(&mut out, node.style.stroke.as_ref());
    push_corner_radius_css(&mut out, node.style.corner_radius);
    push_box_shadow_css(&mut out, &node.effects);
    push_filter_css(&mut out, &node.effects);
    if matches!(node.node_type, NodeType::TextLayer) {
        push_text_css(&mut out, node);
    }
    if !node.visible {
        out.push_str("display: none;\n");
    }
    if out.is_empty() {
        // Always emit *something* so the inspect panel never shows
        // an empty code block.
        out.push_str("/* (node has no visual properties) */\n");
    }
    out
}

/// Emit a space-separated string of Tailwind utility classes that
/// approximate the node's appearance. Falls back to arbitrary-value
/// utilities (`w-[123px]`) for non-standard sizes.
#[must_use]
pub fn node_to_tailwind(node: &Node) -> String {
    let mut tokens: Vec<String> = Vec::new();
    push_size_tw(&mut tokens, node);
    push_position_tw(&mut tokens, node);
    push_opacity_tw(&mut tokens, node);
    push_fill_tw(&mut tokens, &node.style.fill);
    push_stroke_tw(&mut tokens, node.style.stroke.as_ref());
    push_corner_radius_tw(&mut tokens, node.style.corner_radius);
    push_box_shadow_tw(&mut tokens, &node.effects);
    push_filter_tw(&mut tokens, &node.effects);
    if !node.visible {
        tokens.push("hidden".into());
    }
    if tokens.is_empty() {
        tokens.push("/* (node has no visual properties) */".into());
    }
    tokens.join(" ")
}

/// Emit a JSX inline style object literal (the body, including the
/// braces) describing the node's painted appearance. Suitable for
/// pasting into a React component as `style={{ ... }}`.
#[must_use]
pub fn node_to_react_style(node: &Node) -> String {
    let mut entries: Vec<(String, String)> = Vec::new();
    react_push_position(&mut entries, node);
    react_push_size(&mut entries, node);
    react_push_opacity(&mut entries, node);
    react_push_fill(&mut entries, &node.style.fill);
    react_push_stroke(&mut entries, node.style.stroke.as_ref());
    react_push_corner_radius(&mut entries, node.style.corner_radius);
    react_push_box_shadow(&mut entries, &node.effects);
    react_push_filter(&mut entries, &node.effects);
    if matches!(node.node_type, NodeType::TextLayer) {
        react_push_text(&mut entries, node);
    }
    if !node.visible {
        entries.push(("display".into(), "\"none\"".into()));
    }
    if entries.is_empty() {
        return "{\n  // (node has no visual properties)\n}".to_string();
    }
    let mut out = String::from("{\n");
    for (k, v) in entries {
        let _ = writeln!(out, "  {k}: {v},");
    }
    out.push('}');
    out
}

// -- CSS helpers ------------------------------------------------------

fn push_position(out: &mut String, node: &Node) {
    // Only emit absolute positioning when at least one ancestor is
    // expected to be a positioned parent (the inspect mode does not
    // know that, so we emit it unconditionally for non-document /
    // non-artboard nodes — the developer can drop it on paste).
    if matches!(node.node_type, NodeType::Page | NodeType::Artboard) {
        return;
    }
    out.push_str("position: absolute;\n");
    let _ = writeln!(out, "left: {};", px(node.bounds.x));
    let _ = writeln!(out, "top: {};", px(node.bounds.y));
}

fn push_size(out: &mut String, node: &Node) {
    if node.bounds.width > 0.0 {
        let _ = writeln!(out, "width: {};", px(node.bounds.width));
    }
    if node.bounds.height > 0.0 {
        let _ = writeln!(out, "height: {};", px(node.bounds.height));
    }
}

fn push_opacity(out: &mut String, node: &Node) {
    let opacity = node.opacity.clamp(0.0, 1.0);
    if (opacity - 1.0).abs() > f32::EPSILON {
        let _ = writeln!(out, "opacity: {};", trim(f64::from(opacity)));
    }
}

fn push_fill_css(out: &mut String, fill: &FillStyle) {
    match fill {
        FillStyle::None => {}
        FillStyle::Solid(c) => {
            let _ = writeln!(out, "background-color: {};", rgba_css(*c));
        }
        FillStyle::Gradient(g) => {
            let _ = writeln!(out, "background: {};", gradient_css(g));
        }
    }
}

fn push_stroke_css(out: &mut String, stroke: Option<&StrokeStyle>) {
    if let Some(s) = stroke {
        let _ = writeln!(out, "border: {} solid {};", px(s.width), rgba_css(s.color));
        if !s.dash.is_empty() {
            // CSS doesn't support exact dash arrays on borders, so
            // we fall back to a dashed border and leave the array
            // as a comment for the developer.
            out.push_str("border-style: dashed;\n");
            let _ = writeln!(
                out,
                "/* stroke-dasharray: {} */",
                s.dash
                    .iter()
                    .map(|d| trim(*d))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
    }
}

fn push_corner_radius_css(out: &mut String, r: f64) {
    if r > 0.0 {
        let _ = writeln!(out, "border-radius: {};", px(r));
    }
}

fn push_box_shadow_css(out: &mut String, effects: &[Effect]) {
    let shadows: Vec<String> = effects
        .iter()
        .filter_map(|e| match e {
            Effect::Shadow {
                offset_x,
                offset_y,
                blur,
                spread,
                color,
            } => Some(format!(
                "{} {} {} {} {}",
                px(*offset_x),
                px(*offset_y),
                px(*blur),
                px(*spread),
                rgba_css(*color),
            )),
            Effect::Glow { radius, color } => {
                Some(format!("0 0 {} 0 {}", px(*radius), rgba_css(*color)))
            }
            Effect::Blur { .. } => None,
        })
        .collect();
    if !shadows.is_empty() {
        let _ = writeln!(out, "box-shadow: {};", shadows.join(", "));
    }
}

fn push_filter_css(out: &mut String, effects: &[Effect]) {
    let mut filters: Vec<String> = Vec::new();
    for e in effects {
        if let Effect::Blur { radius } = e {
            filters.push(format!("blur({})", px(*radius)));
        }
    }
    if !filters.is_empty() {
        let _ = writeln!(out, "filter: {};", filters.join(" "));
    }
}

fn push_text_css(out: &mut String, node: &Node) {
    // Text styling is stored in node metadata under the `text` key
    // by `kcreate_text`. We surface the common typography fields if
    // present and silently skip the rest.
    if let Some(raw) = node.metadata.get("text") {
        if let Some(family) = raw.get("font_family").and_then(serde_json::Value::as_str) {
            let _ = writeln!(out, "font-family: \"{family}\";");
        }
        if let Some(size) = raw.get("font_size").and_then(serde_json::Value::as_f64) {
            let _ = writeln!(out, "font-size: {};", px(size));
        }
        if let Some(weight) = raw.get("font_weight").and_then(serde_json::Value::as_u64) {
            let _ = writeln!(out, "font-weight: {weight};");
        }
        if let Some(line_height) = raw.get("line_height").and_then(serde_json::Value::as_f64) {
            let _ = writeln!(out, "line-height: {};", trim(line_height));
        }
        if let Some(letter_spacing) = raw.get("letter_spacing").and_then(serde_json::Value::as_f64) {
            let _ = writeln!(out, "letter-spacing: {};", px(letter_spacing));
        }
    }
}

// -- Tailwind helpers -------------------------------------------------

fn push_size_tw(tokens: &mut Vec<String>, node: &Node) {
    if node.bounds.width > 0.0 {
        tokens.push(format!("w-[{}]", px(node.bounds.width)));
    }
    if node.bounds.height > 0.0 {
        tokens.push(format!("h-[{}]", px(node.bounds.height)));
    }
}

fn push_position_tw(tokens: &mut Vec<String>, node: &Node) {
    if matches!(node.node_type, NodeType::Page | NodeType::Artboard) {
        return;
    }
    tokens.push("absolute".into());
    tokens.push(format!("left-[{}]", px(node.bounds.x)));
    tokens.push(format!("top-[{}]", px(node.bounds.y)));
}

fn push_opacity_tw(tokens: &mut Vec<String>, node: &Node) {
    let opacity = node.opacity.clamp(0.0, 1.0);
    if (opacity - 1.0).abs() > f32::EPSILON {
        // Tailwind opacity scale is 0–100.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let pct = (opacity * 100.0).round() as u32;
        tokens.push(format!("opacity-{pct}"));
    }
}

fn push_fill_tw(tokens: &mut Vec<String>, fill: &FillStyle) {
    match fill {
        FillStyle::None => {}
        FillStyle::Solid(c) => {
            tokens.push(format!("bg-[{}]", rgba_hex(*c)));
        }
        FillStyle::Gradient(_) => {
            // Tailwind has gradient utilities but they require a
            // matching color-stop ladder; emit an arbitrary-value
            // background as a fallback.
            tokens.push("bg-gradient-to-br".into());
        }
    }
}

fn push_stroke_tw(tokens: &mut Vec<String>, stroke: Option<&StrokeStyle>) {
    if let Some(s) = stroke {
        tokens.push(format!("border-[{}]", px(s.width)));
        tokens.push(format!("border-[{}]", rgba_hex(s.color)));
        if !s.dash.is_empty() {
            tokens.push("border-dashed".into());
        }
    }
}

fn push_corner_radius_tw(tokens: &mut Vec<String>, r: f64) {
    if r > 0.0 {
        tokens.push(format!("rounded-[{}]", px(r)));
    }
}

fn push_box_shadow_tw(tokens: &mut Vec<String>, effects: &[Effect]) {
    if effects.iter().any(|e| matches!(e, Effect::Shadow { .. })) {
        tokens.push("shadow-lg".into());
    }
}

fn push_filter_tw(tokens: &mut Vec<String>, effects: &[Effect]) {
    if effects.iter().any(|e| matches!(e, Effect::Blur { .. })) {
        tokens.push("blur".into());
    }
}

// -- React inline-style helpers ---------------------------------------

fn react_push_position(entries: &mut Vec<(String, String)>, node: &Node) {
    if matches!(node.node_type, NodeType::Page | NodeType::Artboard) {
        return;
    }
    entries.push(("position".into(), "\"absolute\"".into()));
    entries.push(("left".into(), jsx_px(node.bounds.x)));
    entries.push(("top".into(), jsx_px(node.bounds.y)));
}

fn react_push_size(entries: &mut Vec<(String, String)>, node: &Node) {
    if node.bounds.width > 0.0 {
        entries.push(("width".into(), jsx_px(node.bounds.width)));
    }
    if node.bounds.height > 0.0 {
        entries.push(("height".into(), jsx_px(node.bounds.height)));
    }
}

fn react_push_opacity(entries: &mut Vec<(String, String)>, node: &Node) {
    let opacity = node.opacity.clamp(0.0, 1.0);
    if (opacity - 1.0).abs() > f32::EPSILON {
        entries.push(("opacity".into(), trim(f64::from(opacity))));
    }
}

fn react_push_fill(entries: &mut Vec<(String, String)>, fill: &FillStyle) {
    match fill {
        FillStyle::None => {}
        FillStyle::Solid(c) => {
            entries.push(("backgroundColor".into(), format!("\"{}\"", rgba_css(*c))));
        }
        FillStyle::Gradient(g) => {
            entries.push(("background".into(), format!("\"{}\"", gradient_css(g))));
        }
    }
}

fn react_push_stroke(entries: &mut Vec<(String, String)>, stroke: Option<&StrokeStyle>) {
    if let Some(s) = stroke {
        entries.push((
            "border".into(),
            format!("\"{} solid {}\"", px(s.width), rgba_css(s.color)),
        ));
        if !s.dash.is_empty() {
            entries.push(("borderStyle".into(), "\"dashed\"".into()));
        }
    }
}

fn react_push_corner_radius(entries: &mut Vec<(String, String)>, r: f64) {
    if r > 0.0 {
        entries.push(("borderRadius".into(), jsx_px(r)));
    }
}

fn react_push_box_shadow(entries: &mut Vec<(String, String)>, effects: &[Effect]) {
    let shadows: Vec<String> = effects
        .iter()
        .filter_map(|e| match e {
            Effect::Shadow {
                offset_x,
                offset_y,
                blur,
                spread,
                color,
            } => Some(format!(
                "{} {} {} {} {}",
                px(*offset_x),
                px(*offset_y),
                px(*blur),
                px(*spread),
                rgba_css(*color),
            )),
            Effect::Glow { radius, color } => {
                Some(format!("0 0 {} 0 {}", px(*radius), rgba_css(*color)))
            }
            Effect::Blur { .. } => None,
        })
        .collect();
    if !shadows.is_empty() {
        entries.push(("boxShadow".into(), format!("\"{}\"", shadows.join(", "))));
    }
}

fn react_push_filter(entries: &mut Vec<(String, String)>, effects: &[Effect]) {
    let mut filters: Vec<String> = Vec::new();
    for e in effects {
        if let Effect::Blur { radius } = e {
            filters.push(format!("blur({})", px(*radius)));
        }
    }
    if !filters.is_empty() {
        entries.push(("filter".into(), format!("\"{}\"", filters.join(" "))));
    }
}

fn react_push_text(entries: &mut Vec<(String, String)>, node: &Node) {
    if let Some(raw) = node.metadata.get("text") {
        if let Some(family) = raw.get("font_family").and_then(serde_json::Value::as_str) {
            entries.push(("fontFamily".into(), format!("\"{family}\"")));
        }
        if let Some(size) = raw.get("font_size").and_then(serde_json::Value::as_f64) {
            entries.push(("fontSize".into(), jsx_px(size)));
        }
        if let Some(weight) = raw.get("font_weight").and_then(serde_json::Value::as_u64) {
            entries.push(("fontWeight".into(), weight.to_string()));
        }
        if let Some(line_height) = raw.get("line_height").and_then(serde_json::Value::as_f64) {
            entries.push(("lineHeight".into(), trim(line_height)));
        }
        if let Some(letter_spacing) = raw.get("letter_spacing").and_then(serde_json::Value::as_f64) {
            entries.push(("letterSpacing".into(), jsx_px(letter_spacing)));
        }
    }
}

// -- Bottom utilities -------------------------------------------------

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn channel(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn rgba_css(c: RgbaColor) -> String {
    if (c.a - 1.0).abs() < f32::EPSILON {
        format!(
            "#{:02x}{:02x}{:02x}",
            channel(c.r),
            channel(c.g),
            channel(c.b)
        )
    } else {
        format!(
            "rgba({}, {}, {}, {})",
            channel(c.r),
            channel(c.g),
            channel(c.b),
            trim(f64::from(c.a)),
        )
    }
}

fn rgba_hex(c: RgbaColor) -> String {
    if (c.a - 1.0).abs() < f32::EPSILON {
        format!(
            "#{:02x}{:02x}{:02x}",
            channel(c.r),
            channel(c.g),
            channel(c.b)
        )
    } else {
        format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            channel(c.r),
            channel(c.g),
            channel(c.b),
            channel(c.a),
        )
    }
}

fn gradient_css(g: &GradientKind) -> String {
    match g {
        GradientKind::Linear { from, to, stops } => {
            let stop_strs: Vec<String> = stops
                .iter()
                .map(|s| {
                    format!(
                        "{} {}%",
                        rgba_css(s.color),
                        trim(s.offset.clamp(0.0, 1.0) * 100.0)
                    )
                })
                .collect();
            let dx = to.x - from.x;
            let dy = to.y - from.y;
            let angle_deg = dy.atan2(dx).to_degrees() + 90.0;
            format!(
                "linear-gradient({}deg, {})",
                trim(angle_deg),
                stop_strs.join(", "),
            )
        }
        GradientKind::Radial { stops, .. } => {
            let stop_strs: Vec<String> = stops
                .iter()
                .map(|s| {
                    format!(
                        "{} {}%",
                        rgba_css(s.color),
                        trim(s.offset.clamp(0.0, 1.0) * 100.0)
                    )
                })
                .collect();
            format!("radial-gradient(circle, {})", stop_strs.join(", "))
        }
    }
}

fn px(v: f64) -> String {
    format!("{}px", trim(v))
}

fn jsx_px(v: f64) -> String {
    // JSX inline-style numeric values are interpreted as px by
    // React for the well-known length properties, so we emit a
    // number rather than a string when possible.
    trim(v)
}

fn trim(v: f64) -> String {
    if v.fract().abs() < f64::EPSILON {
        // Cast through i64 to drop the trailing `.0`.
        #[allow(clippy::cast_possible_truncation)]
        let i = v as i64;
        i.to_string()
    } else {
        // Keep two decimal places — enough for sub-pixel offsets,
        // not so many it overruns the panel.
        let s = format!("{v:.2}");
        // Trim trailing zeros / dot.
        let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
        if s.is_empty() {
            "0".to_string()
        } else {
            s
        }
    }
}

// -- Public bundle for the bridge -------------------------------------

/// All three string outputs together. The bridge serializes this
/// struct to JSON and ships it to the inspect panel in one IPC
/// round-trip.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InspectCode {
    pub css: String,
    pub tailwind: String,
    pub react_style: String,
}

/// Compute all three code outputs for `node`.
#[must_use]
pub fn inspect_node(node: &Node) -> InspectCode {
    InspectCode {
        css: node_to_css(node),
        tailwind: node_to_tailwind(node),
        react_style: node_to_react_style(node),
    }
}

// Re-export `Constraint` so downstream consumers don't have to
// import it twice (here for clippy; we *do* use it below in tests).
#[allow(dead_code)]
type _ConstraintBoundsAlias = (Constraint, Bounds);

#[cfg(test)]
mod tests {
    use super::*;
    use kcreate_core::{Bounds, Effect, FillStyle, Node, NodeType, RgbaColor, StrokeStyle};

    fn rect_node() -> Node {
        let mut n = Node::new(NodeType::VectorLayer, "rect");
        n.bounds = Bounds {
            x: 12.0,
            y: 24.0,
            width: 100.0,
            height: 50.0,
        };
        n.style.fill = FillStyle::Solid(RgbaColor {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        });
        n.style.stroke = Some(StrokeStyle {
            color: RgbaColor {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            width: 2.0,
            dash: Vec::new(),
        });
        n.style.corner_radius = 4.0;
        n
    }

    #[test]
    fn css_emits_position_size_fill_stroke_radius() {
        let css = node_to_css(&rect_node());
        assert!(css.contains("position: absolute"));
        assert!(css.contains("left: 12px"));
        assert!(css.contains("top: 24px"));
        assert!(css.contains("width: 100px"));
        assert!(css.contains("height: 50px"));
        assert!(css.contains("background-color: #ff0000"));
        assert!(css.contains("border: 2px solid #000000"));
        assert!(css.contains("border-radius: 4px"));
    }

    #[test]
    fn tailwind_emits_arbitrary_value_tokens() {
        let tw = node_to_tailwind(&rect_node());
        assert!(tw.contains("absolute"));
        assert!(tw.contains("w-[100px]"));
        assert!(tw.contains("h-[50px]"));
        assert!(tw.contains("left-[12px]"));
        assert!(tw.contains("top-[24px]"));
        assert!(tw.contains("bg-[#ff0000]"));
        assert!(tw.contains("rounded-[4px]"));
    }

    #[test]
    fn react_style_emits_object_literal() {
        let js = node_to_react_style(&rect_node());
        assert!(js.starts_with("{\n"));
        assert!(js.contains("position: \"absolute\""));
        assert!(js.contains("left: 12"));
        assert!(js.contains("width: 100"));
        assert!(js.contains("backgroundColor: \"#ff0000\""));
        assert!(js.contains("borderRadius: 4"));
        assert!(js.trim_end().ends_with('}'));
    }

    #[test]
    fn artboard_does_not_emit_absolute_positioning() {
        let mut n = Node::new(NodeType::Artboard, "ab");
        n.bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: 200.0,
            height: 100.0,
        };
        let css = node_to_css(&n);
        assert!(!css.contains("position: absolute"));
        let tw = node_to_tailwind(&n);
        assert!(!tw.split_whitespace().any(|t| t == "absolute"));
    }

    #[test]
    fn shadow_effect_emits_box_shadow() {
        let mut n = rect_node();
        n.effects = vec![Effect::Shadow {
            offset_x: 2.0,
            offset_y: 4.0,
            blur: 8.0,
            spread: 0.0,
            color: RgbaColor {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.5,
            },
        }];
        let css = node_to_css(&n);
        assert!(css.contains("box-shadow: 2px 4px 8px 0px rgba(0, 0, 0, 0.5)"));
    }

    #[test]
    fn invisible_node_emits_hidden() {
        let mut n = rect_node();
        n.visible = false;
        let css = node_to_css(&n);
        assert!(css.contains("display: none"));
        let tw = node_to_tailwind(&n);
        assert!(tw.split_whitespace().any(|t| t == "hidden"));
        let js = node_to_react_style(&n);
        assert!(js.contains("display: \"none\""));
    }

    #[test]
    fn opacity_below_one_emits_property() {
        let mut n = rect_node();
        n.opacity = 0.5;
        let css = node_to_css(&n);
        assert!(css.contains("opacity: 0.5"));
        let tw = node_to_tailwind(&n);
        assert!(tw.split_whitespace().any(|t| t.starts_with("opacity-")));
        let js = node_to_react_style(&n);
        assert!(js.contains("opacity: 0.5"));
    }

    #[test]
    fn text_node_emits_typography_from_metadata() {
        let mut n = Node::new(NodeType::TextLayer, "label");
        n.bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 24.0,
        };
        n.metadata.insert(
            "text".into(),
            serde_json::json!({
                "font_family": "Inter",
                "font_size": 16,
                "font_weight": 600,
                "line_height": 1.25,
                "letter_spacing": 0.5,
            }),
        );
        let css = node_to_css(&n);
        assert!(css.contains("font-family: \"Inter\""));
        assert!(css.contains("font-size: 16px"));
        assert!(css.contains("font-weight: 600"));
        assert!(css.contains("line-height: 1.25"));
        assert!(css.contains("letter-spacing: 0.5px"));
        let js = node_to_react_style(&n);
        assert!(js.contains("fontFamily: \"Inter\""));
        assert!(js.contains("fontSize: 16"));
        assert!(js.contains("fontWeight: 600"));
    }

    #[test]
    fn inspect_node_bundles_all_three_outputs() {
        let r = inspect_node(&rect_node());
        assert!(!r.css.is_empty());
        assert!(!r.tailwind.is_empty());
        assert!(!r.react_style.is_empty());
        assert_ne!(r.css, r.tailwind);
        assert_ne!(r.tailwind, r.react_style);
    }

    #[test]
    fn empty_node_emits_placeholder_block() {
        let mut n = Node::new(NodeType::Page, "page");
        n.style.fill = FillStyle::None;
        n.bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        };
        let css = node_to_css(&n);
        assert!(css.contains("(node has no visual properties)"));
        let tw = node_to_tailwind(&n);
        assert!(tw.contains("(node has no visual properties)"));
        let js = node_to_react_style(&n);
        assert!(js.contains("// (node has no visual properties)"));
    }

    #[test]
    fn fractional_values_trim_trailing_zeros() {
        assert_eq!(trim(1.0), "1");
        assert_eq!(trim(1.5), "1.5");
        assert_eq!(trim(1.50), "1.5");
        assert_eq!(trim(0.0), "0");
        assert_eq!(trim(0.123_456), "0.12");
    }

    #[test]
    fn rgba_with_alpha_emits_rgba_form() {
        let c = RgbaColor {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 0.25,
        };
        assert_eq!(rgba_css(c), "rgba(255, 0, 0, 0.25)");
    }

    #[test]
    fn rgba_opaque_emits_hex_form() {
        let c = RgbaColor {
            r: 0.5,
            g: 0.5,
            b: 0.5,
            a: 1.0,
        };
        assert_eq!(rgba_css(c), "#808080");
    }
}

//! Magic Resize reflow engine ("one design → many sizes").
//!
//! Given a finished design laid out on a *source* frame (an artboard)
//! and a *target* frame of a different size, this engine produces a
//! new set of bounds (and scaled font sizes) for every element so the
//! design reads as if it were *intentionally laid out* for the new
//! size — not naively stretched or letterboxed.
//!
//! # Why not just reuse [`crate::constraints`]?
//!
//! The frame-resize constraint solver ([`apply_constraints`]) answers
//! a narrower question: "the user dragged a frame edge; where does
//! each pinned child go?". Magic Resize answers a broader one: "this
//! whole composition needs to move to a *very* different aspect ratio
//! (square → 9:16 story → A4 poster); reflow it sensibly." That means
//!
//!   * **anchor inference** — most designs don't carry explicit
//!     [`Constraints`]; the engine infers anchoring from geometry (a
//!     full-bleed top bar spans the width and pins to the top; a
//!     centered logo stays centered; a footer sticks to the bottom),
//!   * **coherent scaling** — every element and every glyph scales by
//!     the *same* clamped factor (the geometric mean of the axis
//!     ratios) so type stays readable and proportions stay intact
//!     rather than each axis distorting independently,
//!   * **overflow safety** — children are clamped inside their
//!     reflowed parent so an aspect-ratio change never pushes content
//!     off the artboard.
//!
//! The engine is **pure**: it depends only on [`kcreate_core`] value
//! types, performs no I/O, mutates none of its inputs, and returns a
//! fresh [`ResizeResult`]. The bridge layer owns applying the result
//! to the document graph (and re-running the flex/grid solvers over
//! any auto-layout frames the reflow touched).
//!
//! [`apply_constraints`]: crate::constraints::apply_constraints

use kcreate_core::node::{Bounds, Constraint, Constraints};
use uuid::Uuid;

/// Tunable knobs for a reflow pass. The defaults are calibrated for
/// social/print channel hops (square ⇆ story ⇆ poster) and keep type
/// readable without letting a headline balloon past a poster's frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResizeOptions {
    /// Lower bound on the multiplicative font-size change. Stops type
    /// from collapsing to an illegible size when shrinking to a small
    /// target.
    pub font_scale_min: f64,
    /// Upper bound on the multiplicative font-size change. Stops a
    /// headline from overpowering a large poster.
    pub font_scale_max: f64,
    /// Absolute floor (in px) on any resulting font size.
    pub min_font_px: f64,
    /// Absolute ceiling (in px) on any resulting font size.
    pub max_font_px: f64,
    /// Lower bound on the multiplicative element-size change.
    pub element_scale_min: f64,
    /// Upper bound on the multiplicative element-size change.
    pub element_scale_max: f64,
    /// Fraction of a frame's extent within which an element edge counts
    /// as "pinned" to that edge during geometry-based anchor inference.
    pub edge_fraction: f64,
    /// Coverage (element extent ÷ frame extent) at or above which an
    /// edge-to-edge element is treated as spanning (stretching) the axis.
    pub stretch_coverage: f64,
    /// Tolerance (as a fraction of frame extent) on the leading/trailing
    /// inset difference within which an element counts as centered.
    pub center_tolerance: f64,
}

impl Default for ResizeOptions {
    fn default() -> Self {
        Self {
            font_scale_min: 0.5,
            font_scale_max: 2.5,
            min_font_px: 8.0,
            max_font_px: 320.0,
            element_scale_min: 0.35,
            element_scale_max: 3.5,
            edge_fraction: 0.06,
            stretch_coverage: 0.82,
            center_tolerance: 0.08,
        }
    }
}

/// A node fed into the reflow engine. Mirrors the subset of the
/// document graph the engine actually reasons about: absolute
/// [`Bounds`], anchoring [`Constraints`], an optional font size (set
/// for text layers), and the children to recurse into.
///
/// `bounds` are in the **same absolute coordinate space** as the
/// source frame — exactly what the document graph stores. The engine
/// converts to frame-relative offsets internally.
#[derive(Debug, Clone, PartialEq)]
pub struct ResizeNode {
    pub id: Uuid,
    pub bounds: Bounds,
    pub constraints: Constraints,
    /// `Some(px)` for text layers; `None` for everything else.
    pub font_size: Option<f64>,
    pub children: Vec<Self>,
}

impl ResizeNode {
    /// Convenience constructor for a leaf (no children) with default
    /// constraints and no font. Primarily for tests / callers that
    /// build trees inline.
    #[must_use]
    pub fn leaf(id: Uuid, bounds: Bounds) -> Self {
        Self {
            id,
            bounds,
            constraints: Constraints::default(),
            font_size: None,
            children: Vec::new(),
        }
    }
}

/// The output of a reflow pass: new absolute bounds for every visited
/// node, plus new font sizes for text layers. Emitted in pre-order
/// (parent before child) so a caller applying it top-down sees parents
/// resized before their children.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResizeResult {
    /// `(node_id, new_absolute_bounds)` for every node in the tree.
    pub bounds: Vec<(Uuid, Bounds)>,
    /// `(node_id, new_font_px)` for every text node (those whose
    /// [`ResizeNode::font_size`] was `Some`).
    pub fonts: Vec<(Uuid, f64)>,
}

/// How a single axis of an element should respond to the reflow.
///
/// Derived either from an explicit [`Constraint`] or inferred from the
/// element's geometry relative to its frame (see [`classify_axis`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AxisMode {
    /// Pin both edges: the element spans the frame minus its original
    /// (constant) insets. A full-bleed bar fills the new extent.
    Stretch,
    /// Pin the leading edge (left / top); keep the leading inset
    /// constant and scale the element's own size uniformly.
    Start,
    /// Pin the trailing edge (right / bottom); keep the trailing inset
    /// constant and scale the element's own size uniformly.
    End,
    /// Preserve the element's *fractional* center within the frame and
    /// scale its size uniformly.
    Center,
    /// Free-floating: reposition the leading edge proportionally to the
    /// frame and scale the element's size uniformly.
    Proportional,
}

/// Reflow `roots` (the direct children of the source frame, each with
/// their own descendant subtrees) from `source` to `target`.
///
/// Returns new absolute bounds for every node and new font sizes for
/// text nodes. The input is never mutated.
#[must_use]
pub fn magic_resize(
    roots: &[ResizeNode],
    source: Bounds,
    target: Bounds,
    opts: &ResizeOptions,
) -> ResizeResult {
    let mut out = ResizeResult::default();

    // Degenerate source (zero/negative extent): we can't derive ratios
    // without dividing by zero, so fall back to a rigid translation
    // that re-homes the design at the target origin with sizes intact.
    // This keeps the contract total (no panics / NaNs) on garbage input.
    if !(source.width > 0.0 && source.height > 0.0) {
        let dx = target.x - source.x;
        let dy = target.y - source.y;
        for r in roots {
            translate_subtree(r, dx, dy, &mut out);
        }
        return out;
    }

    let wr = target.width / source.width;
    let hr = target.height / source.height;
    // Geometric mean of the per-axis ratios. Using a single scalar for
    // both axes is what keeps elements and glyphs from distorting when
    // the aspect ratio changes — they grow/shrink uniformly while
    // *anchoring* (not stretching) repositions them.
    let base = (wr * hr).sqrt();
    let element_scale = clamp(base, opts.element_scale_min, opts.element_scale_max);
    let font_scale = clamp(base, opts.font_scale_min, opts.font_scale_max);

    for r in roots {
        reflow(r, source, target, element_scale, font_scale, opts, &mut out);
    }
    out
}

fn reflow(
    node: &ResizeNode,
    parent_old: Bounds,
    parent_new: Bounds,
    element_scale: f64,
    font_scale: f64,
    opts: &ResizeOptions,
    out: &mut ResizeResult,
) {
    let rel_x = node.bounds.x - parent_old.x;
    let rel_y = node.bounds.y - parent_old.y;

    let (h_mode, v_mode) = axis_modes(node, rel_x, rel_y, parent_old, opts);

    let (nx_rel, nw) = solve_axis(
        h_mode,
        rel_x,
        node.bounds.width,
        parent_old.width,
        parent_new.width,
        element_scale,
    );
    let (ny_rel, nh) = solve_axis(
        v_mode,
        rel_y,
        node.bounds.height,
        parent_old.height,
        parent_new.height,
        element_scale,
    );

    let candidate = Bounds {
        x: parent_new.x + nx_rel,
        y: parent_new.y + ny_rel,
        width: nw,
        height: nh,
    };
    // Never let a reflowed child spill outside its reflowed parent —
    // this is what turns an aspect-ratio change into "redistribute the
    // space" rather than "overflow the frame".
    let new_abs = clamp_within(candidate, parent_new);

    out.bounds.push((node.id, new_abs));
    if let Some(fs) = node.font_size {
        out.fonts.push((
            node.id,
            clamp(fs * font_scale, opts.min_font_px, opts.max_font_px),
        ));
    }

    for child in &node.children {
        reflow(
            child,
            node.bounds,
            new_abs,
            element_scale,
            font_scale,
            opts,
            out,
        );
    }
}

/// Pick the [`AxisMode`] for each axis **independently**. An explicit
/// (non-`Fixed`) constraint on an axis wins; an axis left at the
/// default `Fixed` value is inferred from the element's geometry.
///
/// Deciding per-axis — rather than treating the node as fully explicit
/// the moment *either* axis is non-default — is what lets a design that
/// pins only one axis (e.g. `horizontal: Center` with `vertical` left
/// at its default) still get geometry inference on the untouched axis,
/// instead of silently snapping it to the leading edge.
fn axis_modes(
    node: &ResizeNode,
    rel_x: f64,
    rel_y: f64,
    parent_old: Bounds,
    opts: &ResizeOptions,
) -> (AxisMode, AxisMode) {
    (
        axis_mode(
            node.constraints.horizontal,
            rel_x,
            node.bounds.width,
            parent_old.width,
            opts,
        ),
        axis_mode(
            node.constraints.vertical,
            rel_y,
            node.bounds.height,
            parent_old.height,
            opts,
        ),
    )
}

/// Resolve a single axis: honour an explicit (non-`Fixed`) constraint,
/// otherwise infer anchoring from the element's geometry. `Fixed` is
/// the default/unset constraint, so it routes to inference rather than
/// hard-pinning the leading edge.
fn axis_mode(
    c: Constraint,
    origin: f64,
    extent: f64,
    parent: f64,
    opts: &ResizeOptions,
) -> AxisMode {
    match c {
        Constraint::Fixed => classify_axis(origin, extent, parent, opts),
        Constraint::Min => AxisMode::Start,
        Constraint::Max => AxisMode::End,
        Constraint::Center => AxisMode::Center,
        Constraint::Scale => AxisMode::Proportional,
        Constraint::Stretch => AxisMode::Stretch,
    }
}

/// Infer how an axis should reflow from where the element sits within
/// its frame.
fn classify_axis(origin: f64, extent: f64, parent: f64, opts: &ResizeOptions) -> AxisMode {
    if parent <= 0.0 {
        return AxisMode::Start;
    }
    let lead = origin;
    let trail = parent - (origin + extent);
    let coverage = extent / parent;
    let edge = opts.edge_fraction * parent;
    let center_tol = opts.center_tolerance * parent;
    let near_lead = lead <= edge;
    let near_trail = trail <= edge;

    if coverage >= opts.stretch_coverage && near_lead && near_trail {
        AxisMode::Stretch
    } else if near_lead && !near_trail {
        AxisMode::Start
    } else if near_trail && !near_lead {
        AxisMode::End
    } else if (lead - trail).abs() <= center_tol {
        AxisMode::Center
    } else {
        AxisMode::Proportional
    }
}

/// Solve a single axis, returning `(new_relative_origin, new_extent)`.
fn solve_axis(
    mode: AxisMode,
    origin: f64,
    extent: f64,
    parent_old: f64,
    parent_new: f64,
    element_scale: f64,
) -> (f64, f64) {
    if parent_old <= 0.0 {
        return (origin, extent.max(0.0));
    }
    let lead = origin;
    let trail = parent_old - (origin + extent);
    match mode {
        AxisMode::Stretch => {
            // Both edges pinned with their original (constant) insets.
            // A full-bleed element (insets 0) fills the new extent.
            let new_extent = (parent_new - lead - trail).max(0.0);
            (lead, new_extent)
        }
        AxisMode::Start => (lead, (extent * element_scale).max(0.0)),
        AxisMode::End => {
            let new_extent = (extent * element_scale).max(0.0);
            (parent_new - trail - new_extent, new_extent)
        }
        AxisMode::Center => {
            let new_extent = (extent * element_scale).max(0.0);
            let center_frac = (origin + extent * 0.5) / parent_old;
            (center_frac * parent_new - new_extent * 0.5, new_extent)
        }
        AxisMode::Proportional => {
            let pos_frac = origin / parent_old;
            (pos_frac * parent_new, (extent * element_scale).max(0.0))
        }
    }
}

/// Clamp `child` so it lies entirely within `parent`. Sizes are capped
/// to the parent's; the origin is then nudged so the (possibly capped)
/// child sits inside the frame.
fn clamp_within(child: Bounds, parent: Bounds) -> Bounds {
    let w = child.width.clamp(0.0, parent.width.max(0.0));
    let h = child.height.clamp(0.0, parent.height.max(0.0));
    let mut x = child.x;
    let mut y = child.y;
    if x < parent.x {
        x = parent.x;
    }
    if x + w > parent.x + parent.width {
        x = parent.x + parent.width - w;
    }
    if y < parent.y {
        y = parent.y;
    }
    if y + h > parent.y + parent.height {
        y = parent.y + parent.height - h;
    }
    Bounds {
        x,
        y,
        width: w,
        height: h,
    }
}

/// Rigid translation of a whole subtree (used only on the degenerate
/// zero-extent-source fallback). Preserves sizes and fonts.
fn translate_subtree(node: &ResizeNode, dx: f64, dy: f64, out: &mut ResizeResult) {
    out.bounds.push((
        node.id,
        Bounds {
            x: node.bounds.x + dx,
            y: node.bounds.y + dy,
            width: node.bounds.width,
            height: node.bounds.height,
        },
    ));
    if let Some(fs) = node.font_size {
        out.fonts.push((node.id, fs));
    }
    for child in &node.children {
        translate_subtree(child, dx, dy, out);
    }
}

/// `f64::clamp` rejects `min > max` with a panic; this saturating
/// variant is total for any finite inputs and treats an inverted range
/// by preferring `min`.
fn clamp(value: f64, min: f64, max: f64) -> f64 {
    value.max(min).min(max.max(min))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(x: f64, y: f64, w: f64, h: f64) -> Bounds {
        Bounds {
            x,
            y,
            width: w,
            height: h,
        }
    }

    fn find(out: &ResizeResult, id: Uuid) -> &Bounds {
        &out.bounds
            .iter()
            .find(|(i, _)| *i == id)
            .expect("node present in result")
            .1
    }

    fn font_of(out: &ResizeResult, id: Uuid) -> f64 {
        out.fonts
            .iter()
            .find(|(i, _)| *i == id)
            .expect("font present")
            .1
    }

    // 1080² square → 1080×1920 story.
    const SQUARE: Bounds = Bounds {
        x: 0.0,
        y: 0.0,
        width: 1080.0,
        height: 1080.0,
    };
    const STORY: Bounds = Bounds {
        x: 5000.0,
        y: 0.0,
        width: 1080.0,
        height: 1920.0,
    };
    const A4: Bounds = Bounds {
        x: 9000.0,
        y: 0.0,
        width: 2480.0,
        height: 3508.0,
    };

    #[test]
    fn top_bar_stays_at_top_and_spans_new_width() {
        // Full-bleed header: x=0, full width, short, pinned to top.
        let id = Uuid::new_v4();
        let bar = ResizeNode::leaf(id, b(0.0, 0.0, 1080.0, 120.0));
        let out = magic_resize(&[bar], SQUARE, A4, &ResizeOptions::default());
        let nb = find(&out, id);
        // Spans the full target width.
        assert!((nb.x - A4.x).abs() < 1e-6, "x={}", nb.x);
        assert!((nb.width - A4.width).abs() < 1e-6, "width={}", nb.width);
        // Pinned to the top edge.
        assert!((nb.y - A4.y).abs() < 1e-6, "y={}", nb.y);
        // Still well within the top third — a header, not a band down
        // the middle.
        assert!(nb.height < A4.height * 0.25);
    }

    #[test]
    fn centered_logo_stays_centered_on_both_axes() {
        // 200×200 logo centered on the square.
        let id = Uuid::new_v4();
        let logo = ResizeNode::leaf(id, b(440.0, 440.0, 200.0, 200.0));
        let out = magic_resize(&[logo], SQUARE, STORY, &ResizeOptions::default());
        let nb = find(&out, id);
        let cx = nb.x - STORY.x + nb.width * 0.5;
        let cy = nb.y - STORY.y + nb.height * 0.5;
        assert!((cx - STORY.width * 0.5).abs() < 1.0, "cx={cx}");
        assert!((cy - STORY.height * 0.5).abs() < 1.0, "cy={cy}");
    }

    #[test]
    fn footer_sticks_to_bottom() {
        // Footer band near the bottom edge, full width.
        let id = Uuid::new_v4();
        let footer = ResizeNode::leaf(id, b(0.0, 1000.0, 1080.0, 80.0));
        let out = magic_resize(&[footer], SQUARE, STORY, &ResizeOptions::default());
        let nb = find(&out, id);
        // Bottom inset preserved → bottom edge tracks the frame bottom.
        let bottom_gap = (STORY.y + STORY.height) - (nb.y + nb.height);
        assert!(bottom_gap.abs() < 1.0, "bottom_gap={bottom_gap}");
        // Still full width (it spanned the source width).
        assert!((nb.width - STORY.width).abs() < 1e-6);
    }

    #[test]
    fn aspect_change_never_overflows_frame() {
        // A grid of elements scattered across the square; after a hop
        // to a very different aspect ratio none may exit the frame.
        let ids: Vec<Uuid> = (0..9).map(|_| Uuid::new_v4()).collect();
        let mut roots = Vec::new();
        for (i, id) in ids.iter().enumerate() {
            let col = (i % 3) as f64;
            let row = (i / 3) as f64;
            roots.push(ResizeNode::leaf(
                *id,
                b(60.0 + col * 340.0, 60.0 + row * 340.0, 300.0, 300.0),
            ));
        }
        for target in [STORY, A4] {
            let out = magic_resize(&roots, SQUARE, target, &ResizeOptions::default());
            for id in &ids {
                let nb = find(&out, *id);
                assert!(nb.x >= target.x - 1e-6, "left overflow: {}", nb.x);
                assert!(nb.y >= target.y - 1e-6, "top overflow: {}", nb.y);
                assert!(
                    nb.x + nb.width <= target.x + target.width + 1e-6,
                    "right overflow: {}",
                    nb.x + nb.width
                );
                assert!(
                    nb.y + nb.height <= target.y + target.height + 1e-6,
                    "bottom overflow: {}",
                    nb.y + nb.height
                );
            }
        }
    }

    #[test]
    fn fonts_stay_within_clamp_bounds() {
        let opts = ResizeOptions::default();
        let small = Uuid::new_v4();
        let big = Uuid::new_v4();
        let mut tiny = ResizeNode::leaf(small, b(40.0, 40.0, 200.0, 40.0));
        tiny.font_size = Some(6.0); // already below the floor
        let mut head = ResizeNode::leaf(big, b(40.0, 200.0, 1000.0, 200.0));
        head.font_size = Some(200.0); // would blow past the ceiling when scaled
        let out = magic_resize(&[tiny, head], SQUARE, A4, &opts);
        let fs_small = font_of(&out, small);
        let fs_big = font_of(&out, big);
        assert!(fs_small >= opts.min_font_px - 1e-9, "fs_small={fs_small}");
        assert!(fs_big <= opts.max_font_px + 1e-9, "fs_big={fs_big}");
        // Scaling is bounded by font_scale_max regardless of geometry.
        assert!(fs_big <= 200.0 * opts.font_scale_max + 1e-9);
    }

    #[test]
    fn font_scaling_uses_geometric_mean_and_is_clamped() {
        // square → story: wr=1, hr=1.778 → base = sqrt(1.778) ≈ 1.333,
        // within the [0.5, 2.5] clamp.
        let id = Uuid::new_v4();
        let mut t = ResizeNode::leaf(id, b(100.0, 100.0, 400.0, 100.0));
        t.font_size = Some(48.0);
        let out = magic_resize(&[t], SQUARE, STORY, &ResizeOptions::default());
        let expected = 48.0 * (1920.0f64 / 1080.0).sqrt();
        assert!((font_of(&out, id) - expected).abs() < 1e-6);
    }

    #[test]
    fn explicit_stretch_constraint_is_honored() {
        let id = Uuid::new_v4();
        let mut n = ResizeNode::leaf(id, b(100.0, 500.0, 200.0, 50.0));
        n.constraints = Constraints {
            horizontal: Constraint::Stretch,
            vertical: Constraint::Center,
        };
        let out = magic_resize(&[n], SQUARE, A4, &ResizeOptions::default());
        let nb = find(&out, id);
        // Stretch keeps the 100px lead + (1080-300)=780 trail constant,
        // so the new width is target.width - 100 - 780.
        assert!((nb.x - (A4.x + 100.0)).abs() < 1e-6);
        assert!((nb.width - (A4.width - 100.0 - 780.0)).abs() < 1e-6);
    }

    #[test]
    fn explicit_max_constraint_pins_trailing_edge() {
        let id = Uuid::new_v4();
        let mut n = ResizeNode::leaf(id, b(900.0, 100.0, 150.0, 150.0));
        n.constraints = Constraints {
            horizontal: Constraint::Max,
            vertical: Constraint::Min,
        };
        let out = magic_resize(&[n], SQUARE, A4, &ResizeOptions::default());
        let nb = find(&out, id);
        // Trailing gap on x was 1080-900-150 = 30; preserved against
        // the new right edge.
        let right_gap = (A4.x + A4.width) - (nb.x + nb.width);
        assert!((right_gap - 30.0).abs() < 1e-6, "right_gap={right_gap}");
    }

    #[test]
    fn mixed_constraint_infers_the_default_axis() {
        // Horizontal is pinned (Center); vertical is left at its default
        // (Fixed). Per-axis resolution must infer the vertical axis from
        // geometry — a bottom-anchored element sticks to the bottom —
        // rather than the old all-or-nothing path that snapped any
        // default axis to the top the moment the other axis was set.
        let id = Uuid::new_v4();
        let mut n = ResizeNode::leaf(id, b(440.0, 1000.0, 200.0, 80.0));
        n.constraints = Constraints {
            horizontal: Constraint::Center,
            vertical: Constraint::Fixed, // default / unset
        };
        let out = magic_resize(&[n], SQUARE, STORY, &ResizeOptions::default());
        let nb = find(&out, id);
        // Horizontal: stays centered (explicit).
        let cx = nb.x - STORY.x + nb.width * 0.5;
        assert!((cx - STORY.width * 0.5).abs() < 1.0, "cx={cx}");
        // Vertical: inferred bottom-anchor → bottom edge tracks the
        // frame bottom (NOT snapped to the top).
        let bottom_gap = (STORY.y + STORY.height) - (nb.y + nb.height);
        assert!(bottom_gap.abs() < 1.0, "bottom_gap={bottom_gap}");
    }

    #[test]
    fn stretch_coverage_threshold_is_tunable() {
        // A near-full-width bar (90% coverage, edges 5% in) sits above
        // the default stretch threshold. With the default options the
        // engine infers Stretch and the bar spans the new width; raising
        // `stretch_coverage` above the bar's coverage reclassifies it as
        // Center, so it scales about its midpoint instead. This proves
        // the inference thresholds are honoured from `ResizeOptions`
        // rather than baked-in constants.
        const WIDER: Bounds = Bounds {
            x: 7000.0,
            y: 0.0,
            width: 1620.0,
            height: 1080.0,
        };
        let id = Uuid::new_v4();
        let bar = ResizeNode::leaf(id, b(54.0, 0.0, 972.0, 120.0));

        let spanned = magic_resize(
            std::slice::from_ref(&bar),
            SQUARE,
            WIDER,
            &ResizeOptions::default(),
        );
        let sb = find(&spanned, id);
        // Stretch: original 5% insets kept, so it covers most of 1620.
        assert!(sb.width > 1400.0, "stretched width={}", sb.width);
        assert!(
            (sb.x - (WIDER.x + 54.0)).abs() < 1.0,
            "stretched x={}",
            sb.x
        );

        let opts = ResizeOptions {
            stretch_coverage: 0.95,
            ..ResizeOptions::default()
        };
        let centered = magic_resize(std::slice::from_ref(&bar), SQUARE, WIDER, &opts);
        let cb = find(&centered, id);
        // Center: scales about its midpoint → markedly narrower than the
        // stretched result, and stays centered in the new frame.
        assert!(cb.width < 1250.0, "centered width={}", cb.width);
        assert!(
            sb.width > cb.width + 200.0,
            "threshold had no effect: {} vs {}",
            sb.width,
            cb.width
        );
        let cx = cb.x - WIDER.x + cb.width * 0.5;
        assert!((cx - WIDER.width * 0.5).abs() < 1.0, "cx={cx}");
    }

    #[test]
    fn nested_children_stay_within_reflowed_parent() {
        let parent_id = Uuid::new_v4();
        let child_id = Uuid::new_v4();
        let child = ResizeNode::leaf(child_id, b(120.0, 120.0, 200.0, 200.0));
        let parent = ResizeNode {
            id: parent_id,
            bounds: b(100.0, 100.0, 400.0, 400.0),
            constraints: Constraints::default(),
            font_size: None,
            children: vec![child],
        };
        let out = magic_resize(&[parent], SQUARE, A4, &ResizeOptions::default());
        let p = *find(&out, parent_id);
        let c = *find(&out, child_id);
        assert!(c.x >= p.x - 1e-6);
        assert!(c.y >= p.y - 1e-6);
        assert!(c.x + c.width <= p.x + p.width + 1e-6);
        assert!(c.y + c.height <= p.y + p.height + 1e-6);
    }

    #[test]
    fn degenerate_source_translates_without_panicking() {
        let id = Uuid::new_v4();
        let n = ResizeNode::leaf(id, b(10.0, 20.0, 30.0, 40.0));
        let src = b(0.0, 0.0, 0.0, 0.0);
        let out = magic_resize(&[n], src, STORY, &ResizeOptions::default());
        let nb = find(&out, id);
        assert!(nb.x.is_finite() && nb.y.is_finite());
        assert!((nb.width - 30.0).abs() < 1e-9);
        // Translated by target origin.
        assert!((nb.x - (STORY.x + 10.0)).abs() < 1e-9);
    }

    #[test]
    fn identity_resize_is_a_no_op() {
        let id = Uuid::new_v4();
        let n = ResizeNode::leaf(id, b(123.0, 234.0, 321.0, 99.0));
        let out = magic_resize(&[n], SQUARE, SQUARE, &ResizeOptions::default());
        let nb = find(&out, id);
        assert!((nb.x - 123.0).abs() < 1e-6);
        assert!((nb.y - 234.0).abs() < 1e-6);
        assert!((nb.width - 321.0).abs() < 1e-6);
        assert!((nb.height - 99.0).abs() < 1e-6);
    }
}

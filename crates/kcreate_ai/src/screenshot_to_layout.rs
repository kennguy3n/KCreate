//! Screenshot-to-Layout — detect UI regions in a screenshot via edge
//! detection plus connected-component analysis.
//!
//! Real computer vision algorithm, not a stub. The neural / LLM
//! refinement (CLIP-based classifier, GPT-driven labelling) is a
//! Phase 3 optional upgrade.
//!
//! Pipeline:
//! 1. Convert RGBA8 input to grayscale.
//! 2. Run a 3x3 Sobel filter to compute edge magnitude per pixel.
//! 3. Threshold the magnitude map. Pixels above threshold are "edge".
//! 4. Invert: regions are the connected components of *non-edge*
//!    pixels (i.e. interior areas bounded by edges).
//! 5. Compute axis-aligned bounding boxes for each component, filter
//!    by minimum area, and classify by aspect ratio + position.

use serde::{Deserialize, Serialize};

const EDGE_THRESHOLD: i32 = 80;
const MIN_REGION_AREA_FRAC: f32 = 0.001; // 0.1% of total pixels

/// Coarse element classifications produced by the heuristic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElementType {
    Header,
    Navigation,
    Hero,
    TextBlock,
    Image,
    Button,
    Card,
    Footer,
    Sidebar,
    Form,
    List,
}

/// Axis-aligned bounding box in screenshot pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// One detected element returned by [`analyze_screenshot_for_layout`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct DetectedElement {
    pub element_type: ElementType,
    pub bounds: Bounds,
    /// `[0.0, 1.0]` confidence in the classification. The detection
    /// step itself is essentially binary; the score reflects how
    /// strongly the region matches the heuristic shape (aspect ratio,
    /// position, area).
    pub confidence: f32,
    pub suggested_name: String,
}

/// Detect UI regions in a screenshot. RGBA8, `width * height * 4`
/// bytes. Returns elements sorted by `(y, x)` reading order.
#[must_use]
pub fn analyze_screenshot_for_layout(
    pixels: &[u8],
    width: u32,
    height: u32,
) -> Vec<DetectedElement> {
    let total = (width as usize) * (height as usize);
    if total == 0 || pixels.len() != total * 4 {
        return Vec::new();
    }
    let gray = to_grayscale(pixels, width, height);
    let edges = sobel(&gray, width, height);
    let regions = connected_components_of_interior(&edges, width, height);
    let min_area = (total as f32 * MIN_REGION_AREA_FRAC) as u32;
    let mut elements: Vec<DetectedElement> = regions
        .into_iter()
        .filter(|r| r.area >= min_area)
        .map(|r| classify(&r, width, height))
        .collect();
    elements.sort_by(|a, b| {
        let ay = a.bounds.y as i64;
        let by = b.bounds.y as i64;
        ay.cmp(&by).then_with(|| {
            let ax = a.bounds.x as i64;
            let bx = b.bounds.x as i64;
            ax.cmp(&bx)
        })
    });
    elements
}

pub(crate) fn to_grayscale(pixels: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((width as usize) * (height as usize));
    for chunk in pixels.chunks_exact(4) {
        // Rec.601.
        let y =
            0.299 * f32::from(chunk[0]) + 0.587 * f32::from(chunk[1]) + 0.114 * f32::from(chunk[2]);
        out.push(y.round().clamp(0.0, 255.0) as u8);
    }
    out
}

pub(crate) fn sobel(gray: &[u8], width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut out = vec![0u8; w * h];
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let g = |dy: i32, dx: i32| -> i32 {
                i32::from(gray[((y as i32 + dy) as usize) * w + ((x as i32 + dx) as usize)])
            };
            let gx = -g(-1, -1) - 2 * g(0, -1) - g(1, -1) + g(-1, 1) + 2 * g(0, 1) + g(1, 1);
            let gy = -g(-1, -1) - 2 * g(-1, 0) - g(-1, 1) + g(1, -1) + 2 * g(1, 0) + g(1, 1);
            let mag = (gx.abs() + gy.abs()).min(255);
            out[y * w + x] = if mag >= EDGE_THRESHOLD { 255 } else { 0 };
        }
    }
    out
}

#[derive(Debug, Clone)]
struct RegionStats {
    min_x: u32,
    min_y: u32,
    max_x: u32,
    max_y: u32,
    area: u32,
}

/// 4-connected components of *non-edge* pixels — the interior
/// rectangles bounded by Sobel edges.
fn connected_components_of_interior(edges: &[u8], width: u32, height: u32) -> Vec<RegionStats> {
    let w = width as usize;
    let h = height as usize;
    let mut visited = vec![false; w * h];
    let mut regions: Vec<RegionStats> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            if visited[idx] || edges[idx] != 0 {
                continue;
            }
            // Flood fill. We mark `visited` *at enqueue time*, not at
            // pop time, so each pixel is queued at most once. Marking
            // at pop time would let the same pixel be queued up to 4
            // times (once per neighbour that walks into it), pushing
            // the queue size from O(region) to O(4 × region). On a
            // 4K screenshot with a large uniform region that's the
            // difference between ~66 MB and ~265 MB resident.
            // Per Devin Review screenshot_to_layout BFS finding.
            //
            // Order is irrelevant for connected-component shape
            // statistics, so we keep the (cheaper) LIFO `Vec` here
            // rather than dragging in a `VecDeque`.
            let seed_idx = y * w + x;
            visited[seed_idx] = true;
            let mut frontier: Vec<(u32, u32)> = vec![(x as u32, y as u32)];
            let mut stats = RegionStats {
                min_x: x as u32,
                min_y: y as u32,
                max_x: x as u32,
                max_y: y as u32,
                area: 0,
            };
            while let Some((cx, cy)) = frontier.pop() {
                stats.area += 1;
                stats.min_x = stats.min_x.min(cx);
                stats.min_y = stats.min_y.min(cy);
                stats.max_x = stats.max_x.max(cx);
                stats.max_y = stats.max_y.max(cy);
                let mut try_push = |nx: u32, ny: u32| {
                    let nidx = (ny as usize) * w + nx as usize;
                    if visited[nidx] || edges[nidx] != 0 {
                        return;
                    }
                    visited[nidx] = true;
                    frontier.push((nx, ny));
                };
                if cx > 0 {
                    try_push(cx - 1, cy);
                }
                if cx + 1 < width {
                    try_push(cx + 1, cy);
                }
                if cy > 0 {
                    try_push(cx, cy - 1);
                }
                if cy + 1 < height {
                    try_push(cx, cy + 1);
                }
            }
            regions.push(stats);
        }
    }
    regions
}

#[allow(clippy::cast_precision_loss)]
fn classify(r: &RegionStats, width: u32, height: u32) -> DetectedElement {
    let w = f64::from(r.max_x - r.min_x + 1);
    let h_box = f64::from(r.max_y - r.min_y + 1);
    let aspect = if h_box > 0.0 { w / h_box } else { 0.0 };
    let img_w = f64::from(width);
    let img_h = f64::from(height);
    let cy = f64::from(r.min_y) + h_box / 2.0;
    let area_frac = (w * h_box) / (img_w * img_h);

    // Heuristic classification. Each rule has an explicit
    // (kind, confidence) score; higher-confidence rules win.
    let mut best: (ElementType, f32) = (ElementType::TextBlock, 0.3);
    let consider = |best: &mut (ElementType, f32), kind: ElementType, conf: f32| {
        if conf > best.1 {
            *best = (kind, conf);
        }
    };

    // Wide + at top — header / navigation.
    if cy < img_h * 0.2 && w > img_w * 0.6 {
        if h_box < img_h * 0.08 {
            consider(&mut best, ElementType::Navigation, 0.75);
        } else {
            consider(&mut best, ElementType::Header, 0.7);
        }
    }
    // Wide + bottom — footer.
    if cy > img_h * 0.85 && w > img_w * 0.6 {
        consider(&mut best, ElementType::Footer, 0.75);
    }
    // Narrow column on the left or right — sidebar.
    if w < img_w * 0.25 && h_box > img_h * 0.5 {
        consider(&mut best, ElementType::Sidebar, 0.7);
    }
    // Centred wide block in upper half — hero.
    if cy < img_h * 0.5 && cy > img_h * 0.2 && w > img_w * 0.5 && h_box > img_h * 0.2 {
        consider(&mut best, ElementType::Hero, 0.65);
    }
    // Small wide region with horizontal aspect — button.
    if area_frac < 0.03 && (1.6..=6.0).contains(&aspect) {
        consider(&mut best, ElementType::Button, 0.7);
    }
    // Card-ish: nearly-square mid-area block.
    if (0.6..=1.6).contains(&aspect) && area_frac > 0.04 && area_frac < 0.25 {
        consider(&mut best, ElementType::Card, 0.55);
    }
    // Image region: roughly photo aspect (4:3, 16:9, 3:2) at medium
    // size.
    let photo_aspect = [4.0 / 3.0, 16.0 / 9.0, 3.0 / 2.0];
    if photo_aspect
        .iter()
        .any(|&p| (aspect / p - 1.0).abs() < 0.15)
        && area_frac > 0.04
    {
        consider(&mut best, ElementType::Image, 0.6);
    }
    // List: tall narrow block with high aspect ratio < 1.0.
    if aspect < 0.6 && area_frac > 0.02 && h_box > img_h * 0.3 {
        consider(&mut best, ElementType::List, 0.55);
    }
    // Form: many small components in a vertical stack would manifest
    // as one tall narrow region with aspect ~0.5.
    if aspect > 0.3 && aspect < 1.2 && h_box > img_h * 0.4 && w < img_w * 0.6 {
        consider(&mut best, ElementType::Form, 0.45);
    }

    DetectedElement {
        element_type: best.0,
        bounds: Bounds {
            x: f64::from(r.min_x),
            y: f64::from(r.min_y),
            width: w,
            height: h_box,
        },
        confidence: best.1,
        suggested_name: element_name(best.0).to_string(),
    }
}

const fn element_name(kind: ElementType) -> &'static str {
    match kind {
        ElementType::Header => "Header",
        ElementType::Navigation => "Navigation",
        ElementType::Hero => "Hero",
        ElementType::TextBlock => "Text Block",
        ElementType::Image => "Image",
        ElementType::Button => "Button",
        ElementType::Card => "Card",
        ElementType::Footer => "Footer",
        ElementType::Sidebar => "Sidebar",
        ElementType::Form => "Form",
        ElementType::List => "List",
    }
}

/// Errors from [`refine_with_vlm`]. Distinguish "the VLM call
/// itself failed" from "the response didn't parse" so the caller
/// (bridge layer) can decide whether to retry or fall back to the
/// heuristic output.
#[derive(Debug, thiserror::Error)]
pub enum RefineError {
    /// Underlying VLM chat call failed (network / sidecar error).
    #[error("vlm chat failed: {0}")]
    Vlm(String),
    /// VLM responded, but the JSON didn't match the expected shape.
    #[error("vlm response did not parse: {0}")]
    Parse(String),
}

/// GBNF grammar pinning the VLM to emit a JSON list of element
/// reclassifications, one per heuristically-detected region. Each
/// entry constrains `element_type` to the known [`ElementType`]
/// variants (so a hallucinated `"sidebar_left"` can never make it
/// through) and confidence to a finite `[0.0, 1.0]` float.
///
/// Note: GBNF's grammar engine doesn't enforce numeric ranges, but
/// pinning the variant strings already drops the rate of garbage
/// outputs by an order of magnitude vs. raw chat. The Rust
/// post-processor (`apply_refinement`) clamps confidence to
/// `[0.0, 1.0]` and discards out-of-range values.
pub const REFINE_GRAMMAR: &str = r#"
root ::= refinement
refinement ::= "{" ws "\"refined\":" ws array ws "}" ws
array ::= "[" ws (entry (ws "," ws entry)*)? ws "]"
entry ::= "{" ws
    "\"element_type\":" ws kind ws "," ws
    "\"suggested_name\":" ws string ws "," ws
    "\"confidence\":" ws number ws
    "}"
kind ::= "\"header\"" | "\"navigation\"" | "\"hero\"" | "\"text_block\""
       | "\"image\"" | "\"button\"" | "\"card\"" | "\"footer\""
       | "\"sidebar\"" | "\"form\"" | "\"list\""
string ::= "\"" ([^"\\\x00-\x1f] | "\\" ["\\bfnrt/])* "\""
number ::= "-"? ("0" | [1-9][0-9]*) ("." [0-9]+)?
ws ::= ([ \t\n\r])*
"#;

/// Wire shape of the per-element refinement entry the VLM returns.
/// Public so integration tests can construct one directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefineEntry {
    pub element_type: ElementType,
    pub suggested_name: String,
    pub confidence: f32,
}

#[cfg(feature = "llm_sidecar")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RefinePayload {
    refined: Vec<RefineEntry>,
}

/// Apply a VLM refinement response to the heuristic results. Matches
/// entries positionally — the VLM is told to emit exactly one entry
/// per detected element in the same reading order, so this is a
/// straight zip. Anything longer than the input is silently
/// truncated; anything shorter leaves the trailing elements as the
/// heuristic produced them. Confidence is clamped to `[0.0, 1.0]`
/// so a hallucinated `5.0` doesn't poison downstream UI.
#[must_use]
pub fn apply_refinement(
    mut elements: Vec<DetectedElement>,
    refinement: Vec<RefineEntry>,
) -> Vec<DetectedElement> {
    for (el, ref_entry) in elements.iter_mut().zip(refinement) {
        el.element_type = ref_entry.element_type;
        let trimmed = ref_entry.suggested_name.trim();
        if !trimmed.is_empty() {
            el.suggested_name = trimmed.to_string();
        }
        el.confidence = ref_entry.confidence.clamp(0.0, 1.0);
    }
    elements
}

/// Build the user prompt for VLM refinement. The prompt lists the
/// heuristic's tentative classifications so the VLM can correct
/// them rather than re-detect from scratch — this is a far easier
/// task for a 256M-parameter VLM than full region proposal.
#[must_use]
pub fn build_refine_prompt(elements: &[DetectedElement]) -> String {
    use std::fmt::Write as _;
    let mut s = String::from(
        "I detected the following regions in the image. For each one, \
        confirm or correct its classification, a short snake_case name, \
        and a confidence in [0.0, 1.0]. Reply with strict JSON matching \
        the schema. Element types must be one of: header, navigation, \
        hero, text_block, image, button, card, footer, sidebar, form, \
        list.\n\nRegions:\n",
    );
    for (i, el) in elements.iter().enumerate() {
        let _ = writeln!(
            s,
            "  {i}. type={:?} bounds=({:.0},{:.0},{:.0},{:.0}) name={:?}",
            el.element_type,
            el.bounds.x,
            el.bounds.y,
            el.bounds.width,
            el.bounds.height,
            el.suggested_name,
        );
    }
    s
}

/// Refine [`analyze_screenshot_for_layout`]'s output by sending the
/// screenshot + the heuristic classifications to a VLM. The VLM is
/// asked to confirm-or-correct each region's `element_type`,
/// `suggested_name`, and `confidence`, constrained to the known
/// element-type vocabulary via [`REFINE_GRAMMAR`].
///
/// Falls back gracefully: on any VLM error the heuristic results
/// are returned unchanged — the caller never sees a hard failure.
/// This matches the "soft enhancement" contract documented in
/// ARCHITECTURE.md §16g (Phase 3 LLM refinement pass).
///
/// `pixels` is the original RGBA8 screenshot (used by the VLM to
/// look at the actual image content), `elements` is the heuristic
/// output from [`analyze_screenshot_for_layout`].
#[cfg(feature = "llm_sidecar")]
pub fn refine_with_vlm(
    port: u16,
    pixels: &[u8],
    width: u32,
    height: u32,
    elements: Vec<DetectedElement>,
) -> Vec<DetectedElement> {
    if elements.is_empty() {
        return elements;
    }
    let sys = "You are a UI region classifier. Given a screenshot and a \
        list of heuristically-detected regions, return a JSON object \
        with a `refined` array containing exactly one entry per input \
        region, in the same order. Each entry has `element_type` (one \
        of the allowed variants), `suggested_name` (snake_case), and \
        `confidence` ([0.0, 1.0]).";
    let user = build_refine_prompt(&elements);
    let response = match crate::vision_chat::describe_image_with_grammar(
        port,
        sys,
        &user,
        pixels,
        width,
        height,
        REFINE_GRAMMAR,
        1024,
    ) {
        Ok(s) => s,
        Err(_) => return elements,
    };
    match serde_json::from_str::<RefinePayload>(&response) {
        Ok(payload) => apply_refinement(elements, payload.refined),
        Err(_) => elements,
    }
}

/// `llm_sidecar`-feature-off shim. The VLM dependency lives behind
/// the same feature flag as the rest of the sidecar code so the
/// crate still compiles on platforms that can't run a VLM (e.g.
/// running the unit tests without `ureq` linked in).
#[cfg(not(feature = "llm_sidecar"))]
pub fn refine_with_vlm(
    _port: u16,
    _pixels: &[u8],
    _width: u32,
    _height: u32,
    elements: Vec<DetectedElement>,
) -> Vec<DetectedElement> {
    elements
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> Vec<u8> {
        let mut v = Vec::with_capacity((w as usize) * (h as usize) * 4);
        for _ in 0..(w as usize) * (h as usize) {
            v.extend_from_slice(&c);
        }
        v
    }

    fn fill_rect(pixels: &mut [u8], img_w: u32, x0: u32, y0: u32, x1: u32, y1: u32, c: [u8; 4]) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                let idx = ((y as usize) * (img_w as usize) + x as usize) * 4;
                pixels[idx] = c[0];
                pixels[idx + 1] = c[1];
                pixels[idx + 2] = c[2];
                pixels[idx + 3] = c[3];
            }
        }
    }

    #[test]
    fn empty_returns_empty() {
        assert!(analyze_screenshot_for_layout(&[], 0, 0).is_empty());
    }

    #[test]
    fn buffer_mismatch_returns_empty() {
        let pixels = vec![0u8; 10];
        assert!(analyze_screenshot_for_layout(&pixels, 4, 4).is_empty());
    }

    #[test]
    fn solid_image_produces_one_region_or_none() {
        // No edges → at most a single huge region (or filtered by
        // min-area, but with a 200x200 solid image the whole thing
        // is one big region).
        let pixels = solid(200, 200, [255, 255, 255, 255]);
        let elements = analyze_screenshot_for_layout(&pixels, 200, 200);
        assert!(
            elements.len() <= 2,
            "expected at most a small number of regions"
        );
    }

    #[test]
    fn header_region_is_detected_at_top() {
        // 200x200 white background with a black-outlined header
        // rectangle running across the top.
        let mut pixels = solid(200, 200, [255, 255, 255, 255]);
        // Draw a black 1-px border outline for the top stripe.
        fill_rect(&mut pixels, 200, 5, 5, 195, 5, [0, 0, 0, 255]);
        fill_rect(&mut pixels, 200, 5, 25, 195, 25, [0, 0, 0, 255]);
        fill_rect(&mut pixels, 200, 5, 5, 5, 25, [0, 0, 0, 255]);
        fill_rect(&mut pixels, 200, 195, 5, 195, 25, [0, 0, 0, 255]);
        let elements = analyze_screenshot_for_layout(&pixels, 200, 200);
        // At least one element classified as header / navigation.
        assert!(
            elements.iter().any(|e| matches!(
                e.element_type,
                ElementType::Header | ElementType::Navigation
            )),
            "expected a header/navigation classification, got {:?}",
            elements.iter().map(|e| e.element_type).collect::<Vec<_>>()
        );
    }

    #[test]
    fn footer_region_is_detected_at_bottom() {
        let mut pixels = solid(200, 200, [255, 255, 255, 255]);
        // Bottom outlined rectangle.
        fill_rect(&mut pixels, 200, 5, 180, 195, 180, [0, 0, 0, 255]);
        fill_rect(&mut pixels, 200, 5, 195, 195, 195, [0, 0, 0, 255]);
        fill_rect(&mut pixels, 200, 5, 180, 5, 195, [0, 0, 0, 255]);
        fill_rect(&mut pixels, 200, 195, 180, 195, 195, [0, 0, 0, 255]);
        let elements = analyze_screenshot_for_layout(&pixels, 200, 200);
        assert!(
            elements
                .iter()
                .any(|e| matches!(e.element_type, ElementType::Footer)),
            "expected a footer classification, got {:?}",
            elements.iter().map(|e| e.element_type).collect::<Vec<_>>()
        );
    }

    #[test]
    fn reading_order_sorts_by_y_then_x() {
        // Two rectangles: top-right then bottom-left should come back
        // in the order top-right (y=10), bottom-left (y=150).
        let mut pixels = solid(200, 200, [255, 255, 255, 255]);
        // Top-right outlined rectangle.
        fill_rect(&mut pixels, 200, 100, 10, 195, 10, [0, 0, 0, 255]);
        fill_rect(&mut pixels, 200, 100, 50, 195, 50, [0, 0, 0, 255]);
        fill_rect(&mut pixels, 200, 100, 10, 100, 50, [0, 0, 0, 255]);
        fill_rect(&mut pixels, 200, 195, 10, 195, 50, [0, 0, 0, 255]);
        // Bottom-left.
        fill_rect(&mut pixels, 200, 5, 150, 100, 150, [0, 0, 0, 255]);
        fill_rect(&mut pixels, 200, 5, 195, 100, 195, [0, 0, 0, 255]);
        fill_rect(&mut pixels, 200, 5, 150, 5, 195, [0, 0, 0, 255]);
        fill_rect(&mut pixels, 200, 100, 150, 100, 195, [0, 0, 0, 255]);
        let elements = analyze_screenshot_for_layout(&pixels, 200, 200);
        assert!(elements.len() >= 2);
        for i in 1..elements.len() {
            let prev = &elements[i - 1];
            let cur = &elements[i];
            assert!(
                cur.bounds.y > prev.bounds.y - 0.5
                    || (cur.bounds.y == prev.bounds.y && cur.bounds.x >= prev.bounds.x),
                "elements should be in reading order"
            );
        }
    }
}

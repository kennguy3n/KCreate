//! SVG round-trip integration test.
//!
//! Imports an SVG with both basic shapes and path commands, serialises
//! the resulting vector paths back to SVG via `kcreate_vector`, then
//! reimports the output and verifies semantic equivalence (same
//! number of paths, same bounding box).
//!
//! "Bit-identical" is intentionally not required — emitters routinely
//! normalise whitespace, command precision, and shape decomposition.
//! The contract we *do* enforce is: no geometry is lost across the
//! round trip.

use kcreate_vector::{export_svg, import_svg, path::BoundingBox, VectorPath};

const SAMPLE_SVG: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 200">
  <rect x="10" y="10" width="80" height="60" fill="#ff0000"/>
  <circle cx="150" cy="50" r="40" fill="#00ff00"/>
  <path d="M 20 120 L 80 120 L 50 180 Z" fill="#0000ff"/>
  <path d="M 100 120 Q 130 100 160 120 T 200 120" fill="none" stroke="#000"/>
</svg>"##;

#[test]
fn import_export_import_preserves_geometry() {
    let original = import_svg(SAMPLE_SVG.as_bytes()).expect("import");
    assert!(
        !original.is_empty(),
        "sample SVG must yield at least one path",
    );

    // Serialise back to SVG and reimport.
    let regenerated = export_svg(&original, 200.0, 200.0);
    let reimported = import_svg(regenerated.as_bytes()).expect("reimport");

    assert_eq!(
        original.len(),
        reimported.len(),
        "path count must be preserved across round trip\
         \n--- original svg ---\n{SAMPLE_SVG}\n--- regenerated svg ---\n{regenerated}\n",
    );

    // Bounding boxes are the structural fingerprint we care about.
    let original_bbox = combined_bbox(&original);
    let reimported_bbox = combined_bbox(&reimported);

    assert_bbox_close(&original_bbox, &reimported_bbox);
}

fn combined_bbox(paths: &[VectorPath]) -> BoundingBox {
    let mut combined: Option<BoundingBox> = None;
    for p in paths {
        let b = p.bounds();
        combined = Some(combined.map_or(b, |c| BoundingBox {
            min_x: c.min_x.min(b.min_x),
            min_y: c.min_y.min(b.min_y),
            max_x: c.max_x.max(b.max_x),
            max_y: c.max_y.max(b.max_y),
        }));
    }
    combined.unwrap_or(BoundingBox {
        min_x: 0.0,
        min_y: 0.0,
        max_x: 0.0,
        max_y: 0.0,
    })
}

fn assert_bbox_close(a: &BoundingBox, b: &BoundingBox) {
    const EPS: f64 = 1.0; // 1px tolerance — covers float→string→float rounding
    assert!(
        (a.min_x - b.min_x).abs() < EPS
            && (a.min_y - b.min_y).abs() < EPS
            && (a.max_x - b.max_x).abs() < EPS
            && (a.max_y - b.max_y).abs() < EPS,
        "bounding boxes drift > {EPS}px: {a:?} vs {b:?}",
    );
}

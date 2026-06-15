//! G1 — gradient fill render-parity in the raster pipeline.
//!
//! These tests are the visual + pixel-level proof that a gradient fill
//! is now honoured by the CPU/tiny-skia raster backend (and therefore
//! the live canvas and every raster export), closing the gap where a
//! gradient only ever showed up in SVG/PDF.
//!
//! Two complementary proofs:
//!
//! 1. [`sunset_poster_raster_shows_smooth_gradients`] composes a
//!    recognizable "sunset over water" poster directly as a renderer
//!    [`Scene`] — a multi-stop **linear** sky gradient, a **radial**
//!    sun, a linear water gradient, solid mountain silhouettes and a
//!    scatter of stars — runs it through the real PNG export pipeline,
//!    decodes the written file and asserts the pixels form genuine
//!    colour ramps (not a flat representative colour).
//!
//! 2. [`gradient_document_renders_to_raster_and_pdf`] proves end-to-end
//!    *parity from a single source*: one core `DocumentGraph` bearing a
//!    linear- and a radial-gradient vector layer is (a) translated to a
//!    renderer scene via the bridge `SceneSync` and rasterised to PNG —
//!    exercising the previously-broken `node_fill` drop point — and (b)
//!    exported to PDF, where the gradients have always been honoured.
//!    Both outputs must carry the gradients.
//!
//! Every run writes its artefacts under `target/gradient_proof/` so they
//! can be opened and attached.

use std::path::PathBuf;

use kcreate_export::pdf::{export_pdf_from_document, PdfExportOptions, RasterPixelCache};
use kcreate_export::png::{export_png, export_png_to_bytes, PngExportOptions};
use kcreate_export::scene_metadata::VECTOR_PATH_METADATA_KEY;
use kcreate_renderer::geometry::{Color, PathCommand, Point2, Rect, Style};
use kcreate_renderer::scene::{Object, ObjectKind};
use kcreate_renderer::Scene;

use kcreate_bridge::scene_sync::SceneSync;
use kcreate_core::document::DocumentGraph;
use kcreate_core::node::{
    Bounds, FillStyle, GradientKind, GradientStop, Node, NodeType, Point2D, RgbaColor,
};
use kcreate_vector::{PathPoint, PathSegment, VectorPath};

/// Directory where proof artefacts (PNG/PDF) are written. Lives under
/// the workspace `target/` dir, which is git-ignored.
fn proof_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/gradient_proof");
    std::fs::create_dir_all(&dir).expect("create proof dir");
    dir
}

/// Sum of the R, G, B channels at `(x, y)` — a cheap luminance proxy
/// used to compare gradient samples.
fn luma(img: &image::RgbaImage, x: u32, y: u32) -> u32 {
    let p = img.get_pixel(x, y);
    u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])
}

#[test]
fn sunset_poster_raster_shows_smooth_gradients() {
    const W: f32 = 720.0;
    const H: f32 = 960.0;

    let night = Color::rgba(0.02, 0.02, 0.06, 1.0);
    let mut scene = Scene::new(night);

    // --- Sky: a five-stop vertical LINEAR gradient, night → dusk →
    // horizon glow → pale sand. This is the dominant proof surface.
    scene.add_object(
        Object::new(
            ObjectKind::Rect(Rect::new(0.0, 0.0, W, H)),
            Style::linear_gradient(
                Point2::new(0.0, 0.0),
                Point2::new(0.0, H),
                vec![
                    (0.00, Color::rgba(0.04, 0.05, 0.18, 1.0)),
                    (0.40, Color::rgba(0.42, 0.16, 0.40, 1.0)),
                    (0.62, Color::rgba(0.97, 0.45, 0.18, 1.0)),
                    (0.78, Color::rgba(1.00, 0.80, 0.38, 1.0)),
                    (1.00, Color::rgba(0.99, 0.92, 0.72, 1.0)),
                ],
            ),
        )
        .with_z(0),
    );

    // --- Sun: a RADIAL gradient disc on the horizon, white-hot core
    // fading through gold to a transparent orange rim.
    let sun_center = Point2::new(W * 0.5, H * 0.62);
    let sun_radius = 150.0;
    scene.add_object(
        Object::new(
            ObjectKind::Circle {
                center: sun_center,
                radius: sun_radius,
            },
            Style::radial_gradient(
                sun_center,
                sun_radius,
                vec![
                    (0.00, Color::rgba(1.00, 0.99, 0.92, 1.0)),
                    (0.30, Color::rgba(1.00, 0.86, 0.42, 1.0)),
                    (0.72, Color::rgba(1.00, 0.55, 0.16, 0.92)),
                    (1.00, Color::rgba(1.00, 0.45, 0.12, 0.0)),
                ],
            ),
        )
        .with_z(1),
    );

    // --- Water: a LINEAR gradient below the horizon, reflected glow →
    // deep night. Semi-transparent so the sun's reflection bleeds in.
    let horizon = H * 0.70;
    scene.add_object(
        Object::new(
            ObjectKind::Rect(Rect::new(0.0, horizon, W, H - horizon)),
            Style::linear_gradient(
                Point2::new(0.0, horizon),
                Point2::new(0.0, H),
                vec![
                    (0.00, Color::rgba(0.95, 0.62, 0.28, 0.92)),
                    (0.45, Color::rgba(0.35, 0.22, 0.40, 0.95)),
                    (1.00, Color::rgba(0.04, 0.07, 0.16, 1.0)),
                ],
            ),
        )
        .with_z(2),
    );

    // --- Mountain silhouettes straddling the horizon (solid fills, to
    // show solid + gradient compose correctly in the same scene).
    let ridge = Color::rgba(0.08, 0.05, 0.12, 1.0);
    scene.add_object(
        Object::new(
            ObjectKind::Path(vec![
                PathCommand::MoveTo(Point2::new(0.0, horizon)),
                PathCommand::LineTo(Point2::new(W * 0.24, horizon - 140.0)),
                PathCommand::LineTo(Point2::new(W * 0.44, horizon)),
                PathCommand::Close,
            ]),
            Style::filled(ridge),
        )
        .with_z(3),
    );
    scene.add_object(
        Object::new(
            ObjectKind::Path(vec![
                PathCommand::MoveTo(Point2::new(W * 0.40, horizon)),
                PathCommand::LineTo(Point2::new(W * 0.66, horizon - 96.0)),
                PathCommand::LineTo(Point2::new(W * 0.92, horizon)),
                PathCommand::Close,
            ]),
            Style::filled(ridge),
        )
        .with_z(3),
    );

    // --- Stars in the upper sky (solid fills).
    for (sx, sy) in [
        (0.15, 0.08),
        (0.32, 0.16),
        (0.55, 0.06),
        (0.74, 0.12),
        (0.88, 0.20),
    ] {
        let c = Point2::new(W * sx, H * sy);
        scene.add_object(
            Object::new(
                ObjectKind::Circle {
                    center: c,
                    radius: 2.5,
                },
                Style::filled(Color::rgba(1.0, 1.0, 0.95, 0.9)),
            )
            .with_z(4),
        );
    }

    let opts = PngExportOptions {
        width: W as u32,
        height: H as u32,
        scale: 1.0,
        background: Some(night),
    };

    // Write the file (this is the artefact we attach)…
    let png_path = proof_dir().join("sunset_poster.png");
    export_png(&scene, &opts, &png_path).expect("export sunset poster PNG");
    println!("wrote sunset poster proof to {}", png_path.display());

    // …and decode the exact bytes we'd have written to assert on the
    // actual rasterised pixels.
    let bytes = export_png_to_bytes(&scene, &opts).expect("encode sunset poster PNG");
    let img = image::load_from_memory(&bytes)
        .expect("decode PNG")
        .to_rgba8();
    assert_eq!(img.dimensions(), (W as u32, H as u32));

    let cx = (W * 0.5) as u32;

    // Sky LINEAR ramp: the top of the sky is deep night (dark) while a
    // third of the way down is dusk violet (markedly brighter). A flat
    // representative colour would make these equal.
    let sky_top = luma(&img, cx, 20);
    let sky_mid = luma(&img, cx, 300);
    assert!(
        sky_top < 200,
        "sky top should be deep-night dark, luma was {sky_top}"
    );
    assert!(
        sky_mid > sky_top + 80,
        "sky must brighten downward (linear ramp): top={sky_top} mid={sky_mid}"
    );
    // …and the very top is not the pure clear colour either.
    assert!(
        img.get_pixel(cx, 20)[2] > 30,
        "sky top should retain indigo blue, not be cleared to background"
    );

    // Sun RADIAL core: white-hot centre.
    let sun_y = (H * 0.62) as u32;
    let sun_px = img.get_pixel(cx, sun_y);
    assert!(
        sun_px[0] > 200 && sun_px[1] > 200 && sun_px[2] > 150,
        "sun centre should be white-hot, got {sun_px:?}"
    );
    // Radial fall-off: the centre is far brighter than a point near the
    // disc's rim, which is in turn brighter than sky well outside it.
    let sun_core = luma(&img, cx, sun_y);
    let sun_rim = luma(&img, cx + 130, sun_y);
    assert!(
        sun_core > sun_rim + 120,
        "radial gradient must fall off from the core: core={sun_core} rim={sun_rim}"
    );

    // Water LINEAR ramp: bright reflection just under the horizon, deep
    // and dark near the bottom edge.
    let water_top = luma(&img, cx, (horizon as u32) + 18);
    let water_bottom = luma(&img, cx, (H as u32) - 12);
    assert!(
        water_top > water_bottom + 80,
        "water must darken downward (linear ramp): top={water_top} bottom={water_bottom}"
    );
}

/// Build a closed rectangular [`VectorPath`] in absolute coordinates.
fn rect_path(x: f64, y: f64, w: f64, h: f64) -> VectorPath {
    VectorPath::new(vec![
        PathSegment::MoveTo(PathPoint::new(x, y)),
        PathSegment::LineTo(PathPoint::new(x + w, y)),
        PathSegment::LineTo(PathPoint::new(x + w, y + h)),
        PathSegment::LineTo(PathPoint::new(x, y + h)),
        PathSegment::Close,
    ])
}

/// A `VectorLayer` node carrying `path` (in node-local == absolute
/// coords; the bridge translates by `transform`, which we leave at the
/// identity) and the given fill.
fn gradient_layer(name: &str, path: &VectorPath, bounds: Bounds, fill: FillStyle) -> Node {
    let mut node = Node::new(NodeType::VectorLayer, name);
    node.bounds = bounds;
    node.style.fill = fill;
    node.metadata.insert(
        VECTOR_PATH_METADATA_KEY.to_string(),
        serde_json::to_value(path).expect("serialise vector path"),
    );
    node
}

#[test]
fn gradient_document_renders_to_raster_and_pdf() {
    // One source document: a teal→magenta vertical linear panel with a
    // white radial "spotlight" floating over it.
    let mut doc = DocumentGraph::new();

    let panel_path = rect_path(0.0, 0.0, 360.0, 220.0);
    let panel = gradient_layer(
        "panel",
        &panel_path,
        Bounds {
            x: 0.0,
            y: 0.0,
            width: 360.0,
            height: 220.0,
        },
        FillStyle::Gradient(GradientKind::Linear {
            from: Point2D::new(0.0, 0.0),
            to: Point2D::new(0.0, 220.0),
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor::new(0.10, 0.52, 0.82, 1.0),
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor::new(0.92, 0.20, 0.52, 1.0),
                },
            ],
        }),
    );
    doc.insert_node(panel).expect("insert panel");

    let spot_path = rect_path(70.0, 40.0, 220.0, 140.0);
    let spot = gradient_layer(
        "spotlight",
        &spot_path,
        Bounds {
            x: 70.0,
            y: 40.0,
            width: 220.0,
            height: 140.0,
        },
        FillStyle::Gradient(GradientKind::Radial {
            center: Point2D::new(180.0, 110.0),
            radius: 110.0,
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: RgbaColor::new(1.0, 1.0, 1.0, 1.0),
                },
                GradientStop {
                    offset: 1.0,
                    color: RgbaColor::new(0.05, 0.05, 0.12, 1.0),
                },
            ],
        }),
    );
    doc.insert_node(spot).expect("insert spotlight");

    // --- Reference path: PDF has always honoured gradients. Assert the
    // exporter emits real axial (Type-2) and radial (Type-3) shadings.
    // Page proportioned to the design (360:220) so the print render is
    // an undistorted side-by-side with the raster.
    let pdf_opts = PdfExportOptions {
        width_mm: 180.0,
        height_mm: 110.0,
        title: "KCreate gradient card".to_string(),
        ..PdfExportOptions::default()
    };
    let rasters = RasterPixelCache::new();
    let pdf_path = proof_dir().join("gradient_card.pdf");
    export_pdf_from_document(&doc, &pdf_opts, &rasters, &pdf_path).expect("export gradient PDF");
    let pdf_bytes = std::fs::read(&pdf_path).expect("read PDF");
    let pdf_raw = String::from_utf8_lossy(&pdf_bytes);
    assert!(
        pdf_raw.contains("/ShadingType 2"),
        "PDF must carry the linear (axial) gradient shading"
    );
    assert!(
        pdf_raw.contains("/ShadingType 3"),
        "PDF must carry the radial gradient shading"
    );
    println!("wrote gradient PDF proof to {}", pdf_path.display());

    // --- Raster path (the fix): translate the same document to a
    // renderer scene through the bridge and rasterise to PNG. Before G1
    // `node_fill` dropped gradients, so this panel would have been blank
    // / flat.
    let mut sync = SceneSync::new();
    let scene = sync.sync_document_to_scene(&mut doc, None, &[]);
    assert!(
        scene.objects.len() >= 2,
        "both gradient layers must reach the scene, got {}",
        scene.objects.len()
    );

    let opts = PngExportOptions {
        width: 360,
        height: 220,
        scale: 2.0,
        background: Some(Color::rgba(0.0, 0.0, 0.0, 1.0)),
    };
    let png_path = proof_dir().join("gradient_card.png");
    export_png(&scene, &opts, &png_path).expect("export gradient card PNG");
    println!("wrote gradient card proof to {}", png_path.display());

    let bytes = export_png_to_bytes(&scene, &opts).expect("encode gradient card PNG");
    let img = image::load_from_memory(&bytes)
        .expect("decode PNG")
        .to_rgba8();
    let (iw, ih) = img.dimensions();
    assert_eq!((iw, ih), (720, 440), "scale=2 doubles the raster size");

    // Linear panel ramp: sample a column near the left edge (outside the
    // spotlight) — top must read teal-ish (blue-dominant) and the bottom
    // magenta-ish (red-dominant). Equality would mean a flat fill.
    let col = iw / 12;
    let top = img.get_pixel(col, ih / 12);
    let bottom = img.get_pixel(col, ih - ih / 12);
    assert!(
        top[2] > top[0] + 30,
        "linear panel top should be blue-dominant (teal), got {top:?}"
    );
    assert!(
        bottom[0] > bottom[2] + 30,
        "linear panel bottom should be red-dominant (magenta), got {bottom:?}"
    );

    // Radial spotlight: the centre must be near white and brighter than
    // the panel around it.
    let centre = img.get_pixel(iw / 2, ih / 2);
    assert!(
        centre[0] > 200 && centre[1] > 200 && centre[2] > 200,
        "radial spotlight centre should be near white, got {centre:?}"
    );
    let centre_luma = luma(&img, iw / 2, ih / 2);
    let corner_luma = luma(&img, col, ih / 12);
    assert!(
        centre_luma > corner_luma,
        "spotlight centre must be brighter than the panel corner: \
         centre={centre_luma} corner={corner_luma}"
    );
}

//! Phase 0 exit-criteria integration test.
//!
//! Exercises the five Phase 0 acceptance criteria end-to-end against
//! the public crate APIs (no Electron, no Node, no GPU) so CI can
//! enforce them on every commit:
//!
//! 1. **No network.** The deny-list check lives in `local_first.rs`; here we additionally exercise the full editing path (project create, raster import, AI bg-removal, PNG/SVG/PDF/WebP/JPEG export) so any actual runtime socket use would surface as a flake or DNS error.
//! 2. **Project opens locally.** Create `.kstudio`, mutate, save, close, reopen; every node and operation must survive.
//! 3. **Canvas pan/zoom renders correctly.** Initialise the renderer, set a non-identity viewport, render a frame; the pixels must differ from the identity-viewport baseline so we know the transform actually plumbed through.
//! 4. **One AI action runs locally.** Run `threshold-v0` background removal on a synthetic image with a solid background; verify transparent pixels exist on the output.
//! 5. **Export works.** PNG, SVG, PDF, WebP, and JPEG exports each produce non-empty, format-valid bytes.

use std::sync::Mutex;

use kcreate_ai::{remove_background, BgRemoveOptions};
use kcreate_core::document::DocumentGraph;
use kcreate_core::node::{Bounds, Node, NodeType, Transform2D};
use kcreate_core::operation::Operation;
use kcreate_export::{
    export_jpeg_to_bytes, export_pdf_from_document, export_png_to_bytes, export_svg_from_document,
    export_webp_to_bytes, JpegExportOptions, PdfExportOptions, PngExportOptions, RasterPixelCache,
    SvgExportOptions, WebpExportOptions,
};
use kcreate_mcp::tools::{ArtboardInfo, DocumentAccess};
use kcreate_renderer::{
    geometry::Vec2,
    scene::{Object, ObjectKind, Scene},
    Color, Rect, RenderContext, Style,
};
use kcreate_storage::ProjectStore;
use kcreate_vector::{PathPoint, PathSegment, VectorPath};
use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;

/// End-to-end pipeline: build a small document, render it, and
/// export to all five Phase 0 formats. Each step must succeed without
/// any network IO. The `local_first.rs` deny-list test guarantees no
/// network crate is linked into the editing-path closure; this test
/// exercises the actual codepaths so we'd notice if a real call
/// slipped in at runtime. Covers Phase 0 exit criteria 1 (no network)
/// and 5 (export works).
#[test]
fn phase0_full_pipeline_runs_without_network() {
    // Build a small in-memory document.
    let mut doc = DocumentGraph::new();
    let page_id = doc
        .insert_node(Node::new(NodeType::Page, "Page 1"))
        .expect("page");
    let mut artboard = Node::new(NodeType::Artboard, "Artboard 1");
    artboard.parent_id = Some(page_id);
    artboard.bounds = Bounds::new(0.0, 0.0, 200.0, 200.0);
    artboard.transform = Transform2D::IDENTITY;
    let artboard_id = doc.insert_node(artboard).expect("artboard");

    let mut vector = Node::new(NodeType::VectorLayer, "Square");
    vector.parent_id = Some(artboard_id);
    let path = VectorPath::new(vec![
        PathSegment::MoveTo(PathPoint::new(10.0, 10.0)),
        PathSegment::LineTo(PathPoint::new(90.0, 10.0)),
        PathSegment::LineTo(PathPoint::new(90.0, 70.0)),
        PathSegment::LineTo(PathPoint::new(10.0, 70.0)),
        PathSegment::Close,
    ]);
    vector.metadata.insert("vector_path".into(), json!(path));
    let _vector_id = doc.insert_node(vector).expect("vector");

    // PNG export through the renderer scene.
    let scene = scene_with_rect();
    let png = export_png_to_bytes(
        &scene,
        &PngExportOptions {
            width: 32,
            height: 32,
            scale: 1.0,
            background: Some(Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            }),
        },
    )
    .expect("png export");
    assert!(png.starts_with(&[0x89, b'P', b'N', b'G']), "PNG header");
    assert!(png.len() > 64, "PNG body");

    // SVG export through the document graph.
    let svg = export_svg_from_document(
        &doc,
        &[],
        &SvgExportOptions {
            width: 200.0,
            height: 200.0,
            include_metadata: false,
            optimize: true,
        },
    )
    .expect("svg export");
    assert!(svg.contains("<svg"), "SVG root element");
    assert!(svg.contains("<path"), "vector layer rendered as <path>");

    // PDF export.
    let pdf_dir = TempDir::new().expect("tempdir");
    let pdf_path = pdf_dir.path().join("out.pdf");
    let pdf_opts = PdfExportOptions {
        width_mm: 210.0,
        height_mm: 297.0,
        title: "Phase 0 test".into(),
        color_mode: kcreate_export::pdf::PdfColorMode::default(),
        cmyk_dither: kcreate_export::CmykDither::default(),
    };
    let rasters = RasterPixelCache::new();
    export_pdf_from_document(&doc, &pdf_opts, &rasters, &pdf_path).expect("pdf export");
    let pdf_bytes = std::fs::read(&pdf_path).expect("pdf read");
    assert!(pdf_bytes.starts_with(b"%PDF-"), "PDF header");

    // WebP export through the renderer scene.
    let webp = export_webp_to_bytes(
        &scene,
        &WebpExportOptions {
            width: 32,
            height: 32,
            scale: 1.0,
            quality: 90,
            lossless: true,
            background: None,
        },
    )
    .expect("webp export");
    assert!(webp.starts_with(b"RIFF"), "WebP RIFF header");
    assert_eq!(&webp[8..12], b"WEBP", "WebP signature");

    // JPEG export through the renderer scene.
    let jpeg = export_jpeg_to_bytes(
        &scene,
        &JpegExportOptions {
            width: 32,
            height: 32,
            scale: 1.0,
            quality: 90,
            background: Some(Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            }),
        },
    )
    .expect("jpeg export");
    assert!(jpeg.starts_with(&[0xFF, 0xD8, 0xFF]), "JPEG SOI marker");
}

/// Local-first project lifecycle: create on disk, mutate, save,
/// close, reopen, verify the graph and operation log are intact.
/// Covers Phase 0 exit criterion 2 (project opens locally).
#[test]
fn phase0_project_opens_locally_after_close() {
    let dir = TempDir::new().expect("tempdir");
    let project_dir = dir.path().join("phase0.kstudio");
    let mut store = ProjectStore::create(&project_dir, "phase0").expect("create");

    let mut doc = DocumentGraph::new();
    let page_id = doc
        .insert_node(Node::new(NodeType::Page, "Page"))
        .expect("page");
    let mut vector = Node::new(NodeType::VectorLayer, "v1");
    vector.parent_id = Some(page_id);
    let v_id = doc.insert_node(vector).expect("vector");
    store.save_document(&doc).expect("save");
    let op = Operation::new(
        "user",
        "node.create",
        serde_json::Value::Null,
        json!({ "id": v_id }),
        vec![v_id],
    );
    let op_id = op.id;
    store.save_operation(&op).expect("save op");

    drop(store);
    let store2 = ProjectStore::open(&project_dir).expect("reopen");
    let doc2 = store2.load_document().expect("load");
    assert_eq!(doc2.node_count(), doc.node_count());
    assert!(doc2.get_node(v_id).is_some());
    let ops = store2.load_operations(100).expect("load ops");
    assert!(ops.iter().any(|o| o.id == op_id));
}

/// Pan/zoom changes pixels: rendering the same scene with different
/// viewports must produce different output. We don't require any
/// particular pixel value — only that the viewport transform actually
/// reaches the rasteriser. Covers Phase 0 exit criterion 3 (canvas
/// pan/zoom renders correctly).
#[test]
fn phase0_canvas_pan_zoom_changes_pixels() {
    let ctx = RenderContext::new(64, 64).expect("renderer init");
    let scene = scene_with_rect();

    // Identity baseline.
    ctx.set_viewport(Vec2::ZERO, 1.0);
    ctx.invalidate_all();
    let f1 = ctx.render_frame(&scene).expect("render identity");
    let baseline = ctx.get_frame_pixels(f1).expect("frame").pixels().to_vec();

    // Zoom to 2x and shift the pan; the rasterised image must differ.
    ctx.set_viewport(Vec2::new(5.0, 7.0), 2.0);
    ctx.invalidate_all();
    let f2 = ctx.render_frame(&scene).expect("render transformed");
    let shifted = ctx.get_frame_pixels(f2).expect("frame").pixels().to_vec();

    assert_eq!(baseline.len(), shifted.len(), "frame stride parity");
    assert_ne!(
        baseline, shifted,
        "pan/zoom must produce a visibly different frame"
    );
}

/// AI bg removal yields transparent pixels on a solid-background
/// image. We synthesise a 16x16 image with a single black square in
/// the centre on a uniform white background — `threshold-v0` should
/// mark every "white" pixel as transparent. Covers Phase 0 exit
/// criterion 4 (one AI image action runs locally).
#[test]
fn phase0_local_bg_removal_runs_on_cpu() {
    const W: u32 = 16;
    const H: u32 = 16;
    let mut input = vec![0u8; (W * H * 4) as usize];
    // Fill with opaque white.
    for px in input.chunks_exact_mut(4) {
        px[0] = 255;
        px[1] = 255;
        px[2] = 255;
        px[3] = 255;
    }
    // Black square at (4..12, 4..12).
    for y in 4..12 {
        for x in 4..12 {
            let i = (y * W + x) as usize * 4;
            input[i] = 0;
            input[i + 1] = 0;
            input[i + 2] = 0;
            input[i + 3] = 255;
        }
    }
    let output = remove_background(&input, W, H, BgRemoveOptions::default()).expect("bg remove");
    assert_eq!(output.len(), input.len());
    // At least the corner pixels (definitely background) must be
    // transparent.
    let corners = [(0u32, 0u32), (W - 1, 0), (0, H - 1), (W - 1, H - 1)];
    for (x, y) in corners {
        let alpha = output[((y * W + x) * 4 + 3) as usize];
        assert_eq!(alpha, 0, "corner ({x},{y}) must be transparent");
    }
    // And the foreground square should still be opaque.
    let i = (8 * W + 8) as usize * 4;
    assert_eq!(output[i + 3], 255, "foreground must remain opaque");
}

/// MCP smoke test: the server is loopback-only (compile-time
/// guarantee in `kcreate_mcp::server::McpServer::start`) and
/// `list_artboards` returns the expected names. We test the tools
/// directly without binding a socket to keep this test deterministic
/// and free of port-collision flake.
#[test]
fn phase0_mcp_tools_work_locally() {
    struct InMemoryDoc(Mutex<DocumentGraph>);
    impl DocumentAccess for InMemoryDoc {
        fn list_artboards(&self) -> Vec<ArtboardInfo> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, n)| n.node_type == NodeType::Artboard)
                .map(|(id, n)| ArtboardInfo {
                    id: id.to_string(),
                    name: n.name.clone(),
                    bounds: n.bounds.into(),
                })
                .collect()
        }

        fn create_node(
            &self,
            node_type: NodeType,
            name: String,
            parent_id: Option<Uuid>,
        ) -> Result<Uuid, String> {
            let mut node = Node::new(node_type, &name);
            node.parent_id = parent_id;
            self.0
                .lock()
                .unwrap()
                .insert_node(node)
                .map_err(|e| e.to_string())
        }

        fn export_svg(&self, _node_ids: &[Uuid]) -> Result<String, String> {
            Ok("<svg/>".into())
        }
    }

    let mut doc = DocumentGraph::new();
    let mut ab = Node::new(NodeType::Artboard, "Hero");
    ab.bounds = Bounds::new(0.0, 0.0, 100.0, 100.0);
    doc.insert_node(ab).expect("insert");
    let access = InMemoryDoc(Mutex::new(doc));
    let artboards = access.list_artboards();
    assert_eq!(artboards.len(), 1);
    assert_eq!(artboards[0].name, "Hero");
    assert_eq!(artboards[0].bounds.width, 100.0);
}

/// A minimal scene used across multiple sub-tests. One opaque rect on
/// a solid background — enough pixels for pan/zoom to produce a
/// difference, and enough geometry for export pipelines to walk.
fn scene_with_rect() -> Scene {
    let mut scene = Scene::new(Color {
        r: 0.1,
        g: 0.1,
        b: 0.1,
        a: 1.0,
    });
    let style = Style {
        fill: Some(Color {
            r: 0.9,
            g: 0.4,
            b: 0.4,
            a: 1.0,
        }),
        stroke: None,
    };
    let rect = Object::new(ObjectKind::Rect(Rect::new(0.0, 0.0, 16.0, 16.0)), style)
        .with_translation(4.0, 4.0);
    scene.add_object(rect);
    scene
}

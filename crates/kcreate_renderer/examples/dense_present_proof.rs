//! Dense-document present-path proof (workstream I2).
//!
//! Renders a real 5,000- and 10,000-node analytics dashboard, writes a
//! PNG of each so the benchmark numbers are visibly tied to a recognis-
//! able scene (not a blank canvas), and prints a before/after table
//! comparing the legacy whole-framebuffer present against the dirty-rect
//! present for a typical single-element edit.
//!
//! Run with:
//! ```text
//! cargo run -p kcreate_renderer --example dense_present_proof --release
//! ```
//! Output (PNGs + `benchmark_table.md`) lands in `$KCREATE_PROOF_DIR`
//! (default `./proof_out`).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use kcreate_perf::Timeline;
use kcreate_renderer::dense_doc::{build_dense_document, toggle_marker, DenseDoc};
use kcreate_renderer::{initialize, DirtyRect, ObjectId, RenderContext, Scene};
use tiny_skia::{IntSize, Pixmap};

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;
const ITERS: u32 = 48;

/// Per-node-count measurement row.
struct Row {
    nodes: usize,
    build_ms: f64,
    present_full_ms: f64,
    present_dirty_ms: f64,
    bytes_full: usize,
    bytes_dirty: usize,
    dirty: DirtyRect,
    png: PathBuf,
}

fn main() {
    let out_dir = std::env::var("KCREATE_PROOF_DIR")
        .map_or_else(|_| PathBuf::from("proof_out"), PathBuf::from);
    std::fs::create_dir_all(&out_dir).expect("create proof dir");

    let mut rows = Vec::new();
    for &target in &[5_000usize, 10_000usize] {
        rows.push(measure(target, &out_dir));
    }

    let table = render_table(&rows);
    println!("\n{table}");
    let table_path = out_dir.join("benchmark_table.md");
    std::fs::write(&table_path, &table).expect("write table");
    println!("\nWrote {}", table_path.display());
    for row in &rows {
        println!("Wrote {}", row.png.display());
    }
}

fn measure(target: usize, out_dir: &Path) -> Row {
    let DenseDoc {
        mut scene,
        marker_id,
    } = build_dense_document(target, WIDTH as f32, HEIGHT as f32);
    let nodes = scene.objects.len();

    let ctx = initialize(WIDTH, HEIGHT).expect("init renderer");

    // Render the baseline frame and save it as the recognisable proof.
    ctx.invalidate_all();
    let frame = ctx.render_frame(&scene).expect("render base frame");
    let png = out_dir.join(format!("dense_dashboard_{nodes}_nodes.png"));
    {
        let lease = ctx.get_frame_pixels(frame).expect("frame pixels");
        save_png(lease.pixels(), lease.width(), lease.height(), &png);
    }
    // Consume the (full) baseline present so the accumulator is clean
    // and the next edit resolves to a genuine marker-sized dirty rect.
    let _ = ctx.take_present(0.5);

    // Timeline (kcreate_perf) annotation of one representative dirty
    // frame: build phase then present phase.
    let mut timeline = Timeline::start(format!("dense_present_{nodes}"));
    toggle_marker(&mut scene, marker_id, true);
    {
        let _build = timeline.scope("build");
        ctx.invalidate_all();
        ctx.render_frame(&scene).expect("render");
    }
    {
        let _present = timeline.scope("present_dirty");
        let _ = ctx.take_present(0.5).expect("present");
    }
    let report = timeline.finish();
    for phase in &report.phases {
        println!(
            "[{}] {} = {:.3} ms",
            report.name,
            phase.label,
            phase.duration_ns as f64 / 1.0e6,
        );
    }

    let build_ms = time_build(&ctx, &mut scene, marker_id);
    let (present_full_ms, bytes_full, _) = time_present(&ctx, &mut scene, marker_id, 0.0);
    let (present_dirty_ms, bytes_dirty, dirty) = time_present(&ctx, &mut scene, marker_id, 0.5);

    Row {
        nodes,
        build_ms,
        present_full_ms,
        present_dirty_ms,
        bytes_full,
        bytes_dirty,
        dirty,
        png,
    }
}

/// Average `render_frame` time (ms) over `ITERS` single-element edits.
fn time_build(ctx: &RenderContext, scene: &mut Scene, marker_id: ObjectId) -> f64 {
    let mut on = false;
    let start = Instant::now();
    for _ in 0..ITERS {
        on = !on;
        toggle_marker(scene, marker_id, on);
        ctx.invalidate_all();
        ctx.render_frame(scene).expect("render");
    }
    elapsed_ms(start) / f64::from(ITERS)
}

/// Average `take_present` time (ms) in isolation, plus the bytes shipped
/// and dirty rect of the last frame.
fn time_present(
    ctx: &RenderContext,
    scene: &mut Scene,
    marker_id: ObjectId,
    fraction: f32,
) -> (f64, usize, DirtyRect) {
    let mut on = false;
    let mut total_ns: u128 = 0;
    let mut bytes = 0;
    let mut dirty = DirtyRect {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    };
    for _ in 0..ITERS {
        on = !on;
        toggle_marker(scene, marker_id, on);
        ctx.invalidate_all();
        ctx.render_frame(scene).expect("render");
        let t = Instant::now();
        let snap = ctx.take_present(fraction).expect("present");
        total_ns += t.elapsed().as_nanos();
        bytes = snap.bytes.len();
        dirty = snap.dirty;
        std::hint::black_box(snap.bytes.len());
    }
    (total_ns as f64 / f64::from(ITERS) / 1.0e6, bytes, dirty)
}

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_nanos() as f64 / 1.0e6
}

fn save_png(pixels: &[u8], width: u32, height: u32, path: &Path) {
    let size = IntSize::from_wh(width, height).expect("valid size");
    let pixmap = Pixmap::from_vec(pixels.to_vec(), size).expect("pixmap from readback");
    pixmap.save_png(path).expect("save png");
}

fn render_table(rows: &[Row]) -> String {
    let mut s = String::new();
    s.push_str("# I2 — dirty-rect present: before/after (1920x1080)\n\n");
    s.push_str(
        "| Nodes | Build (ms) | Present FULL (ms) | Present DIRTY (ms) | Bytes FULL | Bytes DIRTY | Dirty rect | Bytes reduction | Present speedup | FPS full→dirty |\n",
    );
    s.push_str(
        "| ----: | ---------: | ----------------: | -----------------: | ---------: | ----------: | ---------- | --------------: | --------------: | -------------- |\n",
    );
    for r in rows {
        let total_full = r.build_ms + r.present_full_ms;
        let total_dirty = r.build_ms + r.present_dirty_ms;
        let fps_full = if total_full > 0.0 {
            1000.0 / total_full
        } else {
            0.0
        };
        let fps_dirty = if total_dirty > 0.0 {
            1000.0 / total_dirty
        } else {
            0.0
        };
        let bytes_reduction = if r.bytes_dirty > 0 {
            r.bytes_full as f64 / r.bytes_dirty as f64
        } else {
            0.0
        };
        let present_speedup = if r.present_dirty_ms > 0.0 {
            r.present_full_ms / r.present_dirty_ms
        } else {
            0.0
        };
        let _ = writeln!(
            s,
            "| {nodes} | {build:.3} | {full:.3} | {dirty:.4} | {bf} | {bd} | {dx},{dy} {dw}x{dh} | {red:.0}x | {sp:.1}x | {ff:.0}→{fd:.0} |",
            nodes = r.nodes,
            build = r.build_ms,
            full = r.present_full_ms,
            dirty = r.present_dirty_ms,
            bf = fmt_bytes(r.bytes_full),
            bd = fmt_bytes(r.bytes_dirty),
            dx = r.dirty.x,
            dy = r.dirty.y,
            dw = r.dirty.width,
            dh = r.dirty.height,
            red = bytes_reduction,
            sp = present_speedup,
            ff = fps_full,
            fd = fps_dirty,
        );
    }
    s.push_str(
        "\nBuild is the full CPU re-rasterisation (identical for both paths). \
         The dirty-rect path ships only the changed sub-region, so the per-frame \
         copy + IPC + putImageData payload drops from the whole framebuffer to a \
         few KiB.\n",
    );

    // Full-frame churn (pan / zoom / scroll) is the case the dirty-rect
    // path *cannot* shrink: every pixel moves, so the dirty rect
    // degenerates to the whole framebuffer and the IPC payload is back to
    // `width*height*4`. That is exactly the per-frame structured-clone the
    // shared-memory present path eliminates — the renderer process maps the
    // framebuffer ring and reads it directly, moving 0 bytes over IPC. See
    // `crates/kcreate_bridge/benches/shared_present_dense.rs` for the
    // publish/read timings.
    let full_frame_bytes = WIDTH as usize * HEIGHT as usize * 4;
    let _ = write!(
        s,
        "\n## Full-frame churn (pan / zoom / scroll)\n\n\
         When the whole frame changes, the dirty rect is the entire \
         {w}x{h} surface, so the dirty-rect path degenerates to a full \
         present: **{bytes} per frame** over IPC. The shared-memory present \
         path maps that framebuffer into the renderer process and reads it \
         directly — **0 bytes over IPC per frame** — so present cost stops \
         scaling with the serialized payload. At 60 fps that is \
         {per_sec:.0} MB/s of IPC traffic removed from the critical path.\n",
        w = WIDTH,
        h = HEIGHT,
        bytes = fmt_bytes(full_frame_bytes),
        per_sec = (full_frame_bytes as f64 * 60.0) / (1000.0 * 1000.0),
    );
    s
}

fn fmt_bytes(n: usize) -> String {
    #[allow(clippy::cast_precision_loss)]
    let f = n as f64;
    if f >= 1024.0 * 1024.0 {
        format!("{:.2} MiB", f / (1024.0 * 1024.0))
    } else if f >= 1024.0 {
        format!("{:.1} KiB", f / 1024.0)
    } else {
        format!("{n} B")
    }
}

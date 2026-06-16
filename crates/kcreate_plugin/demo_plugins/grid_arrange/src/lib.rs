//! Grid Arrange — a real KCreate WASM demo plugin.
//!
//! Tidies a scattered selection into a clean grid. The host injects
//! the current selection geometry as JSON via `kcreate_get_input`;
//! the plugin computes a uniform grid layout and emits one
//! `move_node` proposal per node through `kcreate_write_proposal`.
//! The host validates every proposal and folds the accepted batch
//! into a single undoable operation.
//!
//! Sandbox note: this module touches no files, no network, and no
//! DOM — only the four/seven host imports below. All computation is
//! pure arithmetic over the injected geometry.
//!
//! Input contract (built by `plugin_execute_on_selection` in the
//! bridge):
//! ```json
//! {
//!   "nodes": [{"id": "<uuid>", "x": 0.0, "y": 0.0,
//!              "width": 0.0, "height": 0.0}],
//!   "params": {"columns": 3, "gap": 24.0}
//! }
//! ```
//! `columns` and `gap` are optional; when omitted the plugin derives
//! a near-square column count and a 24px gutter.

use serde::Deserialize;

extern "C" {
    fn kcreate_get_input_len() -> i32;
    fn kcreate_get_input(ptr: *mut u8, max_len: i32) -> i32;
    fn kcreate_set_output(ptr: *const u8, len: i32);
    fn kcreate_log(ptr: *const u8, len: i32);
    fn kcreate_write_proposal(ptr: *const u8, len: i32) -> i32;
}

fn host_log(msg: &str) {
    // SAFETY: pointer + length describe a live, initialized byte slice
    // owned by `msg` for the duration of the call; the host only reads.
    unsafe { kcreate_log(msg.as_ptr(), msg.len() as i32) }
}

fn read_input() -> Vec<u8> {
    // SAFETY: no arguments; returns the host-side input length.
    let len = unsafe { kcreate_get_input_len() };
    if len <= 0 {
        return Vec::new();
    }
    let mut buf = vec![0u8; len as usize];
    // SAFETY: `buf` has `len` writable bytes; the host writes at most
    // `len` and returns how many it actually wrote.
    let written = unsafe { kcreate_get_input(buf.as_mut_ptr(), len) };
    buf.truncate(written.max(0) as usize);
    buf
}

fn set_output(msg: &str) {
    // SAFETY: see `host_log`.
    unsafe { kcreate_set_output(msg.as_ptr(), msg.len() as i32) }
}

fn write_proposal(json: &str) -> bool {
    // SAFETY: see `host_log`. Returns 1 when the host accepted the
    // proposal into its pending batch, 0 when it was denied/invalid.
    unsafe { kcreate_write_proposal(json.as_ptr(), json.len() as i32) == 1 }
}

#[derive(Deserialize)]
struct NodeIn {
    id: String,
    #[serde(default)]
    x: f64,
    #[serde(default)]
    y: f64,
    #[serde(default)]
    width: f64,
    #[serde(default)]
    height: f64,
}

#[derive(Deserialize, Default)]
struct Params {
    columns: Option<u32>,
    gap: Option<f64>,
}

#[derive(Deserialize)]
struct Input {
    #[serde(default)]
    nodes: Vec<NodeIn>,
    #[serde(default)]
    params: Params,
}

/// Plugin entry point. Exported as the wasm function `run` and called
/// by the runtime as a `TypedFunc<(), ()>`.
#[no_mangle]
pub extern "C" fn run() {
    let raw = read_input();
    let input: Input = match serde_json::from_slice(&raw) {
        Ok(parsed) => parsed,
        Err(err) => {
            host_log(&format!("grid_arrange: invalid input json: {err}"));
            set_output("{\"moved\":0,\"error\":\"invalid_input\"}");
            return;
        }
    };

    let count = input.nodes.len();
    if count == 0 {
        host_log("grid_arrange: empty selection, nothing to arrange");
        set_output("{\"moved\":0}");
        return;
    }

    let gap = input.params.gap.unwrap_or(24.0).max(0.0);
    let columns = input
        .params
        .columns
        .unwrap_or_else(|| (count as f64).sqrt().ceil() as u32)
        .max(1);

    // Uniform cells sized to the largest selected node so the grid
    // reads as evenly spaced regardless of per-node dimensions.
    let cell_w = input
        .nodes
        .iter()
        .map(|n| n.width)
        .fold(0.0_f64, f64::max)
        .max(1.0);
    let cell_h = input
        .nodes
        .iter()
        .map(|n| n.height)
        .fold(0.0_f64, f64::max)
        .max(1.0);
    // Anchor the grid at the selection's current top-left so the
    // arrangement stays where the user already placed their content.
    let origin_x = input.nodes.iter().map(|n| n.x).fold(f64::INFINITY, f64::min);
    let origin_y = input.nodes.iter().map(|n| n.y).fold(f64::INFINITY, f64::min);

    let mut moved = 0u32;
    for (index, node) in input.nodes.iter().enumerate() {
        let col = (index as u32) % columns;
        let row = (index as u32) / columns;
        let target_x = origin_x + f64::from(col) * (cell_w + gap);
        let target_y = origin_y + f64::from(row) * (cell_h + gap);
        let dx = target_x - node.x;
        let dy = target_y - node.y;
        if dx.abs() < 1e-6 && dy.abs() < 1e-6 {
            continue;
        }
        let proposal = format!(
            "{{\"type\":\"move_node\",\"node_id\":\"{}\",\"dx\":{dx},\"dy\":{dy}}}",
            node.id
        );
        if write_proposal(&proposal) {
            moved += 1;
        }
    }

    set_output(&format!("{{\"moved\":{moved},\"columns\":{columns}}}"));
}

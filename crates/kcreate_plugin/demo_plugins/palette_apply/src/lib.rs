//! Palette Apply — a real KCreate WASM demo plugin.
//!
//! Generates a harmonious color palette and recolors the current
//! selection with it. The host injects the selected node ids as JSON
//! via `kcreate_get_input`; the plugin walks the selection in order,
//! rotating hue by the golden angle (137.508°) so adjacent nodes get
//! visually distinct yet related colors, then emits one `update_node`
//! proposal per node setting a solid fill through
//! `kcreate_write_proposal`. The host folds the accepted batch into a
//! single undoable operation.
//!
//! Sandbox note: no files, no network, no DOM — only the host imports
//! below. The palette is computed purely from the selection order and
//! optional HSL parameters.
//!
//! Input contract (built by `plugin_execute_on_selection` in the
//! bridge):
//! ```json
//! {
//!   "nodes": [{"id": "<uuid>"}],
//!   "params": {"saturation": 0.62, "lightness": 0.55,
//!              "hueOffset": 265.0}
//! }
//! ```
//! All params are optional and clamped to sane ranges.

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
}

#[derive(Deserialize, Default)]
struct Params {
    saturation: Option<f64>,
    lightness: Option<f64>,
    #[serde(rename = "hueOffset", alias = "hue_offset")]
    hue_offset: Option<f64>,
}

#[derive(Deserialize)]
struct Input {
    #[serde(default)]
    nodes: Vec<NodeIn>,
    #[serde(default)]
    params: Params,
}

/// Convert an HSL triple (`h` in degrees `[0,360)`, `s`/`l` in
/// `[0,1]`) into linear-ish sRGB components in `[0,1]`. Standard
/// piecewise HSL→RGB; KCreate's `RgbaColor` stores 0..1 floats.
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (f64, f64, f64) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - ((hp % 2.0) - 1.0).abs());
    let (r1, g1, b1) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    (r1 + m, g1 + m, b1 + m)
}

/// Plugin entry point. Exported as the wasm function `run` and called
/// by the runtime as a `TypedFunc<(), ()>`.
#[no_mangle]
pub extern "C" fn run() {
    let raw = read_input();
    let input: Input = match serde_json::from_slice(&raw) {
        Ok(parsed) => parsed,
        Err(err) => {
            host_log(&format!("palette_apply: invalid input json: {err}"));
            set_output("{\"recolored\":0,\"error\":\"invalid_input\"}");
            return;
        }
    };

    let count = input.nodes.len();
    if count == 0 {
        host_log("palette_apply: empty selection, nothing to recolor");
        set_output("{\"recolored\":0}");
        return;
    }

    let saturation = input.params.saturation.unwrap_or(0.62).clamp(0.0, 1.0);
    let lightness = input.params.lightness.unwrap_or(0.55).clamp(0.0, 1.0);
    // Default origin hue sits in KCreate's brand violet range; each
    // node advances by the golden angle for an even, pleasing spread.
    let hue0 = input.params.hue_offset.unwrap_or(265.0).rem_euclid(360.0);

    let mut recolored = 0u32;
    for (index, node) in input.nodes.iter().enumerate() {
        let hue = (hue0 + (index as f64) * 137.508).rem_euclid(360.0);
        let (r, g, b) = hsl_to_rgb(hue, saturation, lightness);
        let proposal = format!(
            "{{\"type\":\"update_node\",\"node_id\":\"{}\",\"changes\":{{\"fill\":{{\"kind\":\"solid\",\"r\":{r},\"g\":{g},\"b\":{b},\"a\":1.0}}}}}}",
            node.id
        );
        if write_proposal(&proposal) {
            recolored += 1;
        }
    }

    set_output(&format!("{{\"recolored\":{recolored}}}"));
}

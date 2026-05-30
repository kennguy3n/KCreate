//! Phase 10 Block E Task 27 — undo-log memory optimization.
//!
//! The operation log stores complete `before_patch` / `after_patch`
//! JSON snapshots per [`Operation`]. For raster mutations and other
//! large operations this duplicates data: most of the time the two
//! patches differ in only a few fields. This module ships a deterministic
//! diff-and-rebuild encoding so a stored entry only carries
//!
//! * the canonical `before_patch` snapshot, and
//! * a forward diff that, when applied, reproduces `after_patch` byte-
//!   for-byte.
//!
//! Round-trip is exact:
//! [`compress_operation`] → [`expand_operation`] yields an [`Operation`]
//! equal to the input. All metadata fields (id, timestamp, actor,
//! command, affected_nodes, ai_generated, group_id, is_undo) are
//! preserved verbatim.
//!
//! For raster blob payloads embedded in patch JSON, callers should
//! pre-process the patch values with [`replace_blobs_with_refs`] before
//! compressing — that swaps any inline `bytes`-tagged base64 string for
//! a content-addressed reference of the form
//! `{ "__blobRef": "<BLAKE3 hex>" }` and returns the side table of
//! evicted bytes so the caller's blob store can persist them. The
//! inverse [`materialize_blob_refs`] restores the inline form on
//! expansion. Together with the structural diff this drops the average
//! per-op size for raster operations from O(raster bytes) to
//! O(diff size + 64 bytes per blob ref).
//!
//! The encoding is intentionally simple and self-describing — it does
//! not require a third-party `json-patch` crate and keeps
//! `kcreate_core`'s dep tree small. The diff covers JSON objects and
//! arrays at any nesting depth and degrades to a single replace
//! operation for primitive mismatches.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::operation::Operation;

/// One step in a structural JSON diff.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DiffOp {
    /// At `path`, set the value to `value`, creating intermediate
    /// containers as needed. If the terminal segment is a map key it
    /// inserts/overwrites; if it is an array index it overwrites in
    /// place (the index must already exist when applied — the diff
    /// builder never emits an out-of-bounds set for arrays because it
    /// falls back to a whole-array replace on length mismatch).
    Set {
        path: Vec<PathSegment>,
        value: Value,
    },
    /// At `path`, remove the terminal key (object) or splice out the
    /// terminal index (array). No-op if the path does not resolve.
    Remove { path: Vec<PathSegment> },
}

/// One segment of a JSON path. We use a typed enum (instead of a
/// stringly-typed RFC 6901 pointer) so the apply routine never has to
/// guess whether `"0"` means the integer index or a literal map key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PathSegment {
    Key(String),
    Index(usize),
}

/// A compressed representation of an [`Operation`].
///
/// `before_patch` is kept verbatim; `forward_diff` rebuilds
/// `after_patch` when applied to it. All other fields mirror
/// [`Operation`] one-for-one so consumers can serialise this struct
/// directly to disk in place of the full operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompressedOperation {
    pub id: uuid::Uuid,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub actor: String,
    pub command: String,
    pub before_patch: Value,
    pub forward_diff: Vec<DiffOp>,
    pub affected_nodes: Vec<uuid::Uuid>,
    pub ai_generated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<uuid::Uuid>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_undo: bool,
}

/// Build the diff that transforms `from` into `to`.
///
/// The algorithm walks both trees in lockstep:
///
/// * Two objects: per-key diff. Common keys recurse; keys only in
///   `to` emit `Set`, keys only in `from` emit `Remove`.
/// * Two arrays of equal length: per-index diff.
/// * Two arrays of different length, or two non-matching primitive
///   kinds: emit a single `Set` at the current path that replaces the
///   whole subtree. (This is the safe degradation; the apply path
///   handles it correctly.)
/// * Equal values: emit nothing.
#[must_use]
pub fn compute_diff(from: &Value, to: &Value) -> Vec<DiffOp> {
    let mut ops = Vec::new();
    diff_into(from, to, &mut Vec::new(), &mut ops);
    ops
}

fn diff_into(from: &Value, to: &Value, path: &mut Vec<PathSegment>, out: &mut Vec<DiffOp>) {
    if from == to {
        return;
    }
    match (from, to) {
        (Value::Object(a), Value::Object(b)) => {
            // Keys present in `b`: either added or possibly changed.
            for (k, v) in b {
                path.push(PathSegment::Key(k.clone()));
                if let Some(av) = a.get(k) {
                    diff_into(av, v, path, out);
                } else {
                    out.push(DiffOp::Set {
                        path: path.clone(),
                        value: v.clone(),
                    });
                }
                path.pop();
            }
            // Keys present only in `a`: removed.
            for k in a.keys() {
                if !b.contains_key(k) {
                    path.push(PathSegment::Key(k.clone()));
                    out.push(DiffOp::Remove { path: path.clone() });
                    path.pop();
                }
            }
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            for (i, (av, bv)) in a.iter().zip(b.iter()).enumerate() {
                path.push(PathSegment::Index(i));
                diff_into(av, bv, path, out);
                path.pop();
            }
        }
        _ => {
            out.push(DiffOp::Set {
                path: path.clone(),
                value: to.clone(),
            });
        }
    }
}

/// Apply a precomputed diff to `value` in place. Set/Remove operations
/// run in the order returned by [`compute_diff`].
///
/// Safe to call with an empty op list (no-op). Unresolvable paths are
/// silently skipped — this matches the diff builder's contract that
/// every emitted path is reachable from the corresponding `from`
/// snapshot.
pub fn apply_diff(value: &mut Value, ops: &[DiffOp]) {
    for op in ops {
        match op {
            DiffOp::Set { path, value: v } => set_at(value, path, v.clone()),
            DiffOp::Remove { path } => {
                let _ = remove_at(value, path);
            }
        }
    }
}

/// Walk `path` into `root`, replacing the terminal slot with `value`.
/// Creates missing intermediate objects when the terminal slot is an
/// object key whose parent is already an object; otherwise the set is
/// skipped (paths the diff builder emits are always reachable).
fn set_at(root: &mut Value, path: &[PathSegment], value: Value) {
    if path.is_empty() {
        *root = value;
        return;
    }
    let mut node = root;
    for seg in &path[..path.len() - 1] {
        node = match (node, seg) {
            (Value::Object(map), PathSegment::Key(k)) => match map.get_mut(k) {
                Some(child) => child,
                None => return,
            },
            (Value::Array(arr), PathSegment::Index(i)) => match arr.get_mut(*i) {
                Some(child) => child,
                None => return,
            },
            _ => return,
        };
    }
    match (node, path.last().unwrap()) {
        (Value::Object(map), PathSegment::Key(k)) => {
            map.insert(k.clone(), value);
        }
        (Value::Array(arr), PathSegment::Index(i)) => {
            if let Some(slot) = arr.get_mut(*i) {
                *slot = value;
            }
        }
        _ => {}
    }
}

/// Walk `path` into `root` and remove the terminal slot. Returns the
/// removed value when present.
fn remove_at(root: &mut Value, path: &[PathSegment]) -> Option<Value> {
    if path.is_empty() {
        return None;
    }
    let mut node = root;
    for seg in &path[..path.len() - 1] {
        node = match (node, seg) {
            (Value::Object(map), PathSegment::Key(k)) => map.get_mut(k)?,
            (Value::Array(arr), PathSegment::Index(i)) => arr.get_mut(*i)?,
            _ => return None,
        };
    }
    match (node, path.last().unwrap()) {
        (Value::Object(map), PathSegment::Key(k)) => map.remove(k),
        (Value::Array(arr), PathSegment::Index(i)) if *i < arr.len() => Some(arr.remove(*i)),
        _ => None,
    }
}

/// Compress an operation into a [`CompressedOperation`] by replacing
/// `after_patch` with a forward diff against `before_patch`.
#[must_use]
pub fn compress_operation(op: Operation) -> CompressedOperation {
    let forward_diff = compute_diff(&op.before_patch, &op.after_patch);
    CompressedOperation {
        id: op.id,
        timestamp: op.timestamp,
        actor: op.actor,
        command: op.command,
        before_patch: op.before_patch,
        forward_diff,
        affected_nodes: op.affected_nodes,
        ai_generated: op.ai_generated,
        group_id: op.group_id,
        is_undo: op.is_undo,
    }
}

/// Restore an [`Operation`] from its compressed form. Always
/// round-trips with [`compress_operation`].
#[must_use]
pub fn expand_operation(c: CompressedOperation) -> Operation {
    let mut after = c.before_patch.clone();
    apply_diff(&mut after, &c.forward_diff);
    Operation {
        id: c.id,
        timestamp: c.timestamp,
        actor: c.actor,
        command: c.command,
        before_patch: c.before_patch,
        after_patch: after,
        affected_nodes: c.affected_nodes,
        ai_generated: c.ai_generated,
        group_id: c.group_id,
        is_undo: c.is_undo,
    }
}

// ---------------------------------------------------------------------------
// Blob-reference plumbing for raster ops
// ---------------------------------------------------------------------------

/// Marker key used to flag a content-addressed blob reference in a
/// patch JSON tree.
pub const BLOB_REF_MARKER: &str = "__blobRef";
/// Marker key used to flag an inline blob (base64-encoded bytes) in a
/// patch JSON tree.
pub const INLINE_BLOB_MARKER: &str = "__inlineBlob";

/// Walk `value` and replace every inline blob marker
/// (`{"__inlineBlob": "<base64>"}`) larger than `threshold_bytes` with
/// a `{"__blobRef": "<BLAKE3 hex>"}` reference. The returned table maps
/// hex hash → raw bytes; the caller is responsible for persisting it
/// into their blob store before discarding the original `Operation`.
///
/// Smaller inline blobs (below `threshold_bytes`) are kept inline so
/// the round-trip remains exact without a blob-store dependency.
pub fn replace_blobs_with_refs(
    value: &mut Value,
    threshold_bytes: usize,
) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    extract_blobs(value, threshold_bytes, &mut out);
    out
}

fn extract_blobs(value: &mut Value, threshold: usize, out: &mut Vec<(String, Vec<u8>)>) {
    if let Value::Object(map) = value {
        // Detect a single-key inline-blob marker first.
        if map.len() == 1 {
            if let Some(Value::String(b64)) = map.get(INLINE_BLOB_MARKER) {
                if let Ok(bytes) = decode_base64(b64) {
                    if bytes.len() >= threshold {
                        let hash = blake3::hash(&bytes).to_hex().to_string();
                        out.push((hash.clone(), bytes));
                        let mut replacement = Map::new();
                        replacement.insert(BLOB_REF_MARKER.into(), Value::String(hash));
                        *value = Value::Object(replacement);
                        return;
                    }
                }
            }
        }
        for v in map.values_mut() {
            extract_blobs(v, threshold, out);
        }
    } else if let Value::Array(arr) = value {
        for v in arr.iter_mut() {
            extract_blobs(v, threshold, out);
        }
    }
}

/// Inverse of [`replace_blobs_with_refs`]. Walks `value` and replaces
/// every `{"__blobRef": "<hex>"}` marker with the corresponding inline
/// form drawn from `blob_lookup`. Markers whose hash is absent from
/// `blob_lookup` are left in place so the caller can detect the
/// missing-blob condition.
pub fn materialize_blob_refs<F>(value: &mut Value, mut blob_lookup: F)
where
    F: FnMut(&str) -> Option<Vec<u8>>,
{
    materialize_inner(value, &mut blob_lookup);
}

fn materialize_inner<F>(value: &mut Value, blob_lookup: &mut F)
where
    F: FnMut(&str) -> Option<Vec<u8>>,
{
    if let Value::Object(map) = value {
        if map.len() == 1 {
            if let Some(Value::String(hex)) = map.get(BLOB_REF_MARKER) {
                if let Some(bytes) = blob_lookup(hex) {
                    let b64 = encode_base64(&bytes);
                    let mut replacement = Map::new();
                    replacement.insert(INLINE_BLOB_MARKER.into(), Value::String(b64));
                    *value = Value::Object(replacement);
                    return;
                }
            }
        }
        for v in map.values_mut() {
            materialize_inner(v, blob_lookup);
        }
    } else if let Value::Array(arr) = value {
        for v in arr.iter_mut() {
            materialize_inner(v, blob_lookup);
        }
    }
}

// ---------------------------------------------------------------------------
// Base64 (stdlib-style, no extra crate) — we use the `base64` crate
// already pulled in by other crates in the workspace.
// ---------------------------------------------------------------------------

fn decode_base64(s: &str) -> Result<Vec<u8>, ()> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|_| ())
}

fn encode_base64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_op(before: Value, after: Value) -> Operation {
        Operation::new("user", "test", before, after, vec![])
    }

    #[test]
    fn empty_diff_when_patches_are_equal() {
        let v = json!({"a": 1, "b": [1, 2, 3]});
        let d = compute_diff(&v, &v);
        assert!(d.is_empty(), "no ops when from == to, got {d:?}");
    }

    #[test]
    fn object_key_added_emits_set() {
        let from = json!({"a": 1});
        let to = json!({"a": 1, "b": 2});
        let d = compute_diff(&from, &to);
        assert_eq!(d.len(), 1);
        assert!(matches!(
            &d[0],
            DiffOp::Set { path, value } if path == &vec![PathSegment::Key("b".into())] && value == &json!(2)
        ));
    }

    #[test]
    fn object_key_removed_emits_remove() {
        let from = json!({"a": 1, "b": 2});
        let to = json!({"a": 1});
        let d = compute_diff(&from, &to);
        assert_eq!(d.len(), 1);
        assert!(matches!(
            &d[0],
            DiffOp::Remove { path } if path == &vec![PathSegment::Key("b".into())]
        ));
    }

    #[test]
    fn object_value_changed_emits_set() {
        let from = json!({"a": 1, "b": 2});
        let to = json!({"a": 1, "b": 99});
        let d = compute_diff(&from, &to);
        assert_eq!(d.len(), 1);
        assert!(matches!(
            &d[0],
            DiffOp::Set { path, value } if path == &vec![PathSegment::Key("b".into())] && value == &json!(99)
        ));
    }

    #[test]
    fn nested_object_change_emits_nested_path() {
        let from = json!({"a": {"x": {"y": 1}}});
        let to = json!({"a": {"x": {"y": 2}}});
        let d = compute_diff(&from, &to);
        assert_eq!(d.len(), 1);
        assert_eq!(
            d[0],
            DiffOp::Set {
                path: vec![
                    PathSegment::Key("a".into()),
                    PathSegment::Key("x".into()),
                    PathSegment::Key("y".into())
                ],
                value: json!(2)
            }
        );
    }

    #[test]
    fn same_length_array_diffs_per_index() {
        let from = json!({"arr": [1, 2, 3]});
        let to = json!({"arr": [1, 5, 3]});
        let d = compute_diff(&from, &to);
        assert_eq!(d.len(), 1);
        assert_eq!(
            d[0],
            DiffOp::Set {
                path: vec![PathSegment::Key("arr".into()), PathSegment::Index(1)],
                value: json!(5)
            }
        );
    }

    #[test]
    fn different_length_array_falls_back_to_full_replace() {
        let from = json!({"arr": [1, 2, 3]});
        let to = json!({"arr": [1, 2]});
        let d = compute_diff(&from, &to);
        assert_eq!(d.len(), 1);
        assert!(matches!(
            &d[0],
            DiffOp::Set { path, value } if path == &vec![PathSegment::Key("arr".into())] && value == &json!([1, 2])
        ));
    }

    #[test]
    fn apply_diff_reproduces_target() {
        let from = json!({"a": 1, "b": {"c": "old"}, "d": [1, 2, 3]});
        let to = json!({"a": 1, "b": {"c": "new", "e": true}, "d": [1, 9, 3]});
        let d = compute_diff(&from, &to);
        let mut rebuilt = from;
        apply_diff(&mut rebuilt, &d);
        assert_eq!(rebuilt, to);
    }

    #[test]
    fn compress_expand_round_trips_metadata_and_payload() {
        let before = json!({"x": 1, "y": [1, 2, 3]});
        let after = json!({"x": 2, "y": [1, 9, 3], "added": true});
        let op = make_op(before.clone(), after.clone());
        let original = op.clone();
        let restored = expand_operation(compress_operation(op));
        // Every metadata field preserved verbatim.
        assert_eq!(original.id, restored.id);
        assert_eq!(original.timestamp, restored.timestamp);
        assert_eq!(original.actor, restored.actor);
        assert_eq!(original.command, restored.command);
        assert_eq!(original.affected_nodes, restored.affected_nodes);
        assert_eq!(original.ai_generated, restored.ai_generated);
        assert_eq!(original.group_id, restored.group_id);
        assert_eq!(original.is_undo, restored.is_undo);
        // Payload restored exactly.
        assert_eq!(restored.before_patch, before);
        assert_eq!(restored.after_patch, after);
    }

    #[test]
    fn compression_shrinks_realistic_raster_edit() {
        // Realistic raster edit: a large shared "settings" blob
        // dominates the payload and only one nested field changes.
        // Compressed encoding must avoid re-emitting the shared blob.
        let mut shared = serde_json::Map::new();
        for i in 0..200 {
            shared.insert(format!("k{i}"), Value::String("payload".repeat(8)));
        }
        let shared = Value::Object(shared);
        let before = json!({
            "node_id": "00000000-0000-0000-0000-000000000001",
            "version": 1,
            "settings": &shared,
            "metadata": {"k": "v1"},
        });
        let after = json!({
            "node_id": "00000000-0000-0000-0000-000000000001",
            "version": 2,
            "settings": shared,
            "metadata": {"k": "v2"},
        });
        let op = make_op(before, after);
        let raw = serde_json::to_vec(&op).expect("ser raw");
        let compressed = compress_operation(op);
        let comp_bytes = serde_json::to_vec(&compressed).expect("ser compressed");
        // The shared 200-key blob appears once in compressed form
        // (in `before_patch`) but twice in raw — so compressed must be
        // dramatically smaller.
        // Generous threshold: compressed must be at most ~55% of raw
        // (raw is ~2x compressed because the shared blob appears in
        // both before_patch and after_patch). Real-world rasters
        // beat this by orders of magnitude — this is the regression
        // floor.
        let ratio = (comp_bytes.len() as f64) / (raw.len() as f64);
        assert!(
            ratio < 0.55,
            "compressed/raw ratio {ratio:.3} must be < 0.55 \
             (compressed={}, raw={})",
            comp_bytes.len(),
            raw.len(),
        );
    }

    #[test]
    fn blob_extraction_replaces_large_inline_bytes_with_ref() {
        // Inline blob > threshold → swapped for hash ref.
        let big = vec![0xABu8; 1024];
        let mut value = json!({
            "raster": { INLINE_BLOB_MARKER: encode_base64(&big) },
            "small": { INLINE_BLOB_MARKER: encode_base64(&[1u8, 2, 3]) },
        });
        let table = replace_blobs_with_refs(&mut value, 256);
        assert_eq!(table.len(), 1, "only the large blob should be evicted");
        assert_eq!(table[0].1, big);
        // Big slot now carries a blob ref.
        let raster = value.get("raster").and_then(Value::as_object).expect("obj");
        assert!(raster.contains_key(BLOB_REF_MARKER));
        // Small slot retained.
        let small = value.get("small").and_then(Value::as_object).expect("obj");
        assert!(small.contains_key(INLINE_BLOB_MARKER));
    }

    #[test]
    fn materialize_blob_refs_restores_inline_form() {
        let big = vec![0xCDu8; 512];
        let mut value = json!({
            "raster": { INLINE_BLOB_MARKER: encode_base64(&big) },
        });
        let table = replace_blobs_with_refs(&mut value, 128);
        assert_eq!(table.len(), 1);
        let (hash, bytes) = table.into_iter().next().unwrap();
        // Restore via the side table.
        materialize_blob_refs(&mut value, |h| (h == hash).then(|| bytes.clone()));
        let raster = value.get("raster").and_then(Value::as_object).expect("obj");
        let inline = raster
            .get(INLINE_BLOB_MARKER)
            .and_then(Value::as_str)
            .expect("inline");
        assert_eq!(decode_base64(inline).unwrap(), big);
    }

    #[test]
    fn missing_blob_lookup_leaves_marker_in_place() {
        let big = vec![0xFFu8; 600];
        let mut value = json!({
            "raster": { INLINE_BLOB_MARKER: encode_base64(&big) },
        });
        let _table = replace_blobs_with_refs(&mut value, 128);
        // Lookup that always returns None → marker untouched.
        materialize_blob_refs(&mut value, |_| None);
        let raster = value.get("raster").and_then(Value::as_object).expect("obj");
        assert!(raster.contains_key(BLOB_REF_MARKER));
    }

    #[test]
    fn nested_inline_blob_inside_array_is_extracted() {
        let big = vec![0x77u8; 800];
        let mut value = json!({
            "layers": [
                {"name": "raster", "pixels": { INLINE_BLOB_MARKER: encode_base64(&big) }},
                {"name": "vector"},
            ]
        });
        let table = replace_blobs_with_refs(&mut value, 256);
        assert_eq!(table.len(), 1);
        // Navigate down: layers[0].pixels.__blobRef
        let pixels = value
            .get("layers")
            .and_then(|a| a.as_array())
            .and_then(|a| a.first())
            .and_then(|o| o.get("pixels"))
            .and_then(Value::as_object)
            .expect("nested pixels");
        assert!(pixels.contains_key(BLOB_REF_MARKER));
    }
}

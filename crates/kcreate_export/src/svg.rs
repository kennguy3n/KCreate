//! SVG export driven by the [`DocumentGraph`].
//!
//! Walks the supplied document, collects every vector layer in
//! `node_ids` (or all vector layers, when `node_ids` is empty), reads
//! its embedded `VectorPath` from `metadata["vector_path"]`, and hands
//! the whole list to `kcreate_vector::svg_export`.
//!
//! Vector path data is stored in node metadata (not core types) so
//! `kcreate_core` doesn't depend on `kcreate_vector`. This keeps the
//! crate graph one-directional: core → vector → export.

use kcreate_core::document::DocumentGraph;
use kcreate_core::node::NodeType;
use kcreate_vector::{export_svg, BoundingBox, VectorPath};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// SVG export options.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct SvgExportOptions {
    /// Output canvas width in user units. `0` means "fit to content".
    pub width: f64,
    /// Output canvas height in user units. `0` means "fit to content".
    pub height: f64,
    /// Reserved for future use — currently ignored.
    pub include_metadata: bool,
    /// Reserved for future use — currently ignored.
    pub optimize: bool,
}

/// Metadata key on a `VectorLayer` node that holds the serialized
/// `VectorPath`. Storage and export layers agree on this key.
pub const VECTOR_PATH_METADATA_KEY: &str = "vector_path";

/// Errors from SVG export.
#[derive(Debug, Error)]
pub enum SvgDocumentExportError {
    #[error("node not found: {0}")]
    NodeNotFound(Uuid),
    #[error("node {0} is not a vector layer")]
    NotVectorLayer(Uuid),
    #[error("node {0} is missing `{VECTOR_PATH_METADATA_KEY}` metadata")]
    MissingVectorPath(Uuid),
    #[error("node {0} has invalid `{VECTOR_PATH_METADATA_KEY}` metadata: {1}")]
    InvalidVectorPath(Uuid, String),
}

/// Walk `document`, gather vector paths from `node_ids` (or every
/// vector layer when the list is empty), and emit a complete SVG
/// document.
///
/// `width` / `height` of `0` are interpreted as "fit the union of the
/// included paths' bounds".
pub fn export_svg_from_document(
    document: &DocumentGraph,
    node_ids: &[Uuid],
    options: &SvgExportOptions,
) -> Result<String, SvgDocumentExportError> {
    let paths = collect_paths(document, node_ids)?;
    let (w, h) = resolve_dimensions(&paths, options);
    Ok(export_svg(&paths, w, h))
}

fn collect_paths(
    document: &DocumentGraph,
    node_ids: &[Uuid],
) -> Result<Vec<VectorPath>, SvgDocumentExportError> {
    if node_ids.is_empty() {
        // Walk the entire document — descend from each root in DFS
        // order so output order matches z-order.
        let mut out = Vec::new();
        for root in document.root_ids() {
            push_subtree(document, *root, &mut out)?;
        }
        Ok(out)
    } else {
        let mut out = Vec::with_capacity(node_ids.len());
        for id in node_ids {
            let node = document
                .get_node(*id)
                .ok_or(SvgDocumentExportError::NodeNotFound(*id))?;
            if node.node_type != NodeType::VectorLayer {
                return Err(SvgDocumentExportError::NotVectorLayer(*id));
            }
            out.push(read_vector_path(*id, node)?);
        }
        Ok(out)
    }
}

fn push_subtree(
    document: &DocumentGraph,
    id: Uuid,
    out: &mut Vec<VectorPath>,
) -> Result<(), SvgDocumentExportError> {
    let node = document
        .get_node(id)
        .ok_or(SvgDocumentExportError::NodeNotFound(id))?;
    if !node.visible {
        return Ok(());
    }
    if node.node_type == NodeType::VectorLayer {
        // Missing vector data on a vector layer is a logic bug; surface
        // it instead of silently skipping.
        out.push(read_vector_path(id, node)?);
    }
    for child in &node.children {
        push_subtree(document, *child, out)?;
    }
    Ok(())
}

fn read_vector_path(
    id: Uuid,
    node: &kcreate_core::node::Node,
) -> Result<VectorPath, SvgDocumentExportError> {
    let raw = node
        .metadata
        .get(VECTOR_PATH_METADATA_KEY)
        .ok_or(SvgDocumentExportError::MissingVectorPath(id))?;
    serde_json::from_value::<VectorPath>(raw.clone())
        .map_err(|e| SvgDocumentExportError::InvalidVectorPath(id, e.to_string()))
}

fn resolve_dimensions(paths: &[VectorPath], options: &SvgExportOptions) -> (f64, f64) {
    if options.width > 0.0 && options.height > 0.0 {
        return (options.width, options.height);
    }
    let mut bb = BoundingBox::empty();
    for p in paths {
        bb = bb.union(p.bounds());
    }
    let w = if options.width > 0.0 {
        options.width
    } else {
        bb.width().max(1.0)
    };
    let h = if options.height > 0.0 {
        options.height
    } else {
        bb.height().max(1.0)
    };
    (w, h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kcreate_core::node::{Node, NodeType};
    use kcreate_vector::{PathPoint, PathSegment};

    fn make_doc_with_rect() -> (DocumentGraph, Uuid) {
        let mut doc = DocumentGraph::new();
        let page = doc
            .insert_node(Node::new(NodeType::Page, "Page 1"))
            .expect("page");
        let mut layer = Node::new(NodeType::VectorLayer, "Rect");
        layer.parent_id = Some(page);
        let path = VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(0.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(10.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(10.0, 10.0)),
            PathSegment::LineTo(PathPoint::new(0.0, 10.0)),
            PathSegment::Close,
        ]);
        layer.metadata.insert(
            VECTOR_PATH_METADATA_KEY.to_string(),
            serde_json::to_value(&path).expect("path json"),
        );
        let layer_id = doc.insert_node(layer).expect("layer");
        (doc, layer_id)
    }

    #[test]
    fn exports_single_node() {
        let (doc, layer_id) = make_doc_with_rect();
        let svg = export_svg_from_document(&doc, &[layer_id], &SvgExportOptions::default())
            .expect("export");
        assert!(svg.contains("<svg"));
        assert!(svg.contains("M0 0"));
        assert!(svg.contains('Z'));
    }

    #[test]
    fn exports_entire_document_when_ids_empty() {
        let (doc, _) = make_doc_with_rect();
        let svg =
            export_svg_from_document(&doc, &[], &SvgExportOptions::default()).expect("export");
        assert!(svg.contains("<path"));
    }

    #[test]
    fn missing_node_errors() {
        let (doc, _) = make_doc_with_rect();
        let bogus = Uuid::new_v4();
        let err = export_svg_from_document(&doc, &[bogus], &SvgExportOptions::default())
            .expect_err("must err");
        assert!(matches!(err, SvgDocumentExportError::NodeNotFound(_)));
    }

    #[test]
    fn non_vector_node_errors() {
        let mut doc = DocumentGraph::new();
        let page = doc
            .insert_node(Node::new(NodeType::Page, "p"))
            .expect("page");
        let err = export_svg_from_document(&doc, &[page], &SvgExportOptions::default())
            .expect_err("must err");
        assert!(matches!(err, SvgDocumentExportError::NotVectorLayer(_)));
    }

    #[test]
    fn missing_vector_path_metadata_errors() {
        let mut doc = DocumentGraph::new();
        let layer = doc
            .insert_node(Node::new(NodeType::VectorLayer, "broken"))
            .expect("layer");
        let err = export_svg_from_document(&doc, &[layer], &SvgExportOptions::default())
            .expect_err("must err");
        assert!(matches!(err, SvgDocumentExportError::MissingVectorPath(_)));
    }

    #[test]
    fn invisible_node_skipped_in_full_walk() {
        let (mut doc, layer_id) = make_doc_with_rect();
        let layer = doc.get_node_mut(layer_id).expect("layer");
        layer.visible = false;
        let svg =
            export_svg_from_document(&doc, &[], &SvgExportOptions::default()).expect("export");
        assert!(
            !svg.contains("<path"),
            "invisible vector layer should be omitted, got: {svg}"
        );
    }

    #[test]
    fn dimensions_fit_to_content_when_zero() {
        let (doc, layer_id) = make_doc_with_rect();
        let svg = export_svg_from_document(&doc, &[layer_id], &SvgExportOptions::default())
            .expect("export");
        assert!(svg.contains("viewBox=\"0 0 10 10\""));
    }
}

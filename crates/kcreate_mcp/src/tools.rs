//! Handlers for the three Phase 0 MCP tools.
//!
//! The MCP crate **does not** own the document graph (that lives in
//! `kcreate_bridge::document`). Tools accept a [`DocumentAccess`]
//! trait-object so callers can plug in whichever locking discipline
//! they use. The bridge implements [`DocumentAccess`] over its
//! process-global workspace mutex.

use kcreate_core::node::{Bounds, NodeType};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::protocol::codes;

/// Object-safe access to the live document graph.
///
/// `kcreate_mcp` deliberately depends only on `kcreate_core` (graph
/// types) and `kcreate_export` (SVG export). The bridge plugs in a
/// concrete implementation when starting the server.
pub trait DocumentAccess: Send + Sync {
    /// List every artboard's id, name, and bounds.
    fn list_artboards(&self) -> Vec<ArtboardInfo>;

    /// Insert a freshly-named node of the given type and return its
    /// generated id.
    fn create_node(
        &self,
        node_type: NodeType,
        name: String,
        parent_id: Option<Uuid>,
    ) -> Result<Uuid, String>;

    /// Export the requested node(s) to an SVG document.
    fn export_svg(&self, node_ids: &[Uuid]) -> Result<String, String>;
}

/// `list_artboards` response item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtboardInfo {
    pub id: String,
    pub name: String,
    pub bounds: BoundsDto,
}

/// Public bounds DTO (serialises as `{x, y, width, height}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundsDto {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl From<Bounds> for BoundsDto {
    fn from(b: Bounds) -> Self {
        Self {
            x: b.x,
            y: b.y,
            width: b.width,
            height: b.height,
        }
    }
}

/// `list_artboards`
pub fn handle_list_artboards(access: &dyn DocumentAccess) -> Result<Value, (i32, String)> {
    Ok(json!(access.list_artboards()))
}

/// `create_node` params.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateNodeParams {
    pub parent_id: Option<String>,
    pub node_type: String,
    pub name: String,
}

/// `create_node`
pub fn handle_create_node(
    access: &dyn DocumentAccess,
    params: Value,
) -> Result<Value, (i32, String)> {
    let p: CreateNodeParams = serde_json::from_value(params)
        .map_err(|e| (codes::INVALID_PARAMS, format!("invalid params: {e}")))?;
    let node_type = parse_node_type(&p.node_type).ok_or((
        codes::INVALID_PARAMS,
        format!("unknown node_type: {}", p.node_type),
    ))?;
    let parent_id = match p.parent_id.as_deref() {
        Some(s) if !s.is_empty() => Some(
            Uuid::parse_str(s)
                .map_err(|e| (codes::INVALID_PARAMS, format!("bad parent_id: {e}")))?,
        ),
        _ => None,
    };
    let id = access
        .create_node(node_type, p.name, parent_id)
        .map_err(|e| (codes::INTERNAL_ERROR, e))?;
    Ok(json!({ "id": id.to_string() }))
}

/// `export_artboard` params.
#[derive(Debug, Clone, Deserialize)]
pub struct ExportArtboardParams {
    pub id: String,
    /// Phase 0 supports `"svg"`. PNG/PDF support requires renderer
    /// access and lives on the bridge crate, not this one.
    pub format: String,
    pub path: String,
}

/// `export_artboard`
pub fn handle_export_artboard(
    access: &dyn DocumentAccess,
    params: Value,
) -> Result<Value, (i32, String)> {
    let p: ExportArtboardParams = serde_json::from_value(params)
        .map_err(|e| (codes::INVALID_PARAMS, format!("invalid params: {e}")))?;
    let node_id =
        Uuid::parse_str(&p.id).map_err(|e| (codes::INVALID_PARAMS, format!("bad id: {e}")))?;

    match p.format.as_str() {
        "svg" => {
            let svg = access
                .export_svg(&[node_id])
                .map_err(|e| (codes::INTERNAL_ERROR, e))?;
            std::fs::write(&p.path, svg)
                .map_err(|e| (codes::INTERNAL_ERROR, format!("write failed: {e}")))?;
            Ok(json!({ "path": p.path, "format": "svg" }))
        }
        other => Err((
            codes::INVALID_PARAMS,
            format!("unsupported format: {other} (Phase 0 supports: svg)"),
        )),
    }
}

fn parse_node_type(s: &str) -> Option<NodeType> {
    Some(match s {
        "page" | "Page" => NodeType::Page,
        "artboard" | "Artboard" => NodeType::Artboard,
        "group" | "GroupLayer" => NodeType::GroupLayer,
        "vector" | "VectorLayer" => NodeType::VectorLayer,
        "raster" | "RasterLayer" => NodeType::RasterLayer,
        "text" | "TextLayer" => NodeType::TextLayer,
        "component" | "ComponentLayer" => NodeType::ComponentLayer,
        "layout" | "LayoutFrame" => NodeType::LayoutFrame,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kcreate_core::document::DocumentGraph;
    use kcreate_core::node::Node;
    use kcreate_export::svg::SvgExportOptions;
    use parking_lot::Mutex;
    use std::sync::Arc;

    struct InMemoryDoc(Mutex<DocumentGraph>);
    impl DocumentAccess for InMemoryDoc {
        fn list_artboards(&self) -> Vec<ArtboardInfo> {
            self.0
                .lock()
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
            let mut node = Node::new(node_type, name);
            node.parent_id = parent_id;
            self.0.lock().insert_node(node).map_err(|e| e.to_string())
        }
        fn export_svg(&self, node_ids: &[Uuid]) -> Result<String, String> {
            kcreate_export::svg::export_svg_from_document(
                &self.0.lock(),
                node_ids,
                &SvgExportOptions::default(),
            )
            .map_err(|e| e.to_string())
        }
    }

    #[test]
    fn list_artboards_returns_artboards_only() {
        let mut doc = DocumentGraph::new();
        let ab = Node::new(NodeType::Artboard, "Frame 1");
        let v = Node::new(NodeType::VectorLayer, "vec");
        let ab_id = doc.insert_node(ab).expect("insert");
        let _ = doc.insert_node(v).expect("insert");
        let access: Arc<dyn DocumentAccess> = Arc::new(InMemoryDoc(Mutex::new(doc)));
        let result = handle_list_artboards(&*access).expect("ok");
        let arr = result.as_array().expect("array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], ab_id.to_string());
    }

    #[test]
    fn create_node_via_tool() {
        let doc = DocumentGraph::new();
        let access: Arc<dyn DocumentAccess> = Arc::new(InMemoryDoc(Mutex::new(doc)));
        let result = handle_create_node(&*access, json!({"node_type": "artboard", "name": "Hero"}))
            .expect("ok");
        assert!(result["id"].as_str().is_some());
    }
}

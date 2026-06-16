//! Handlers for the MCP tool surface — from the low-level graph
//! primitives (`list_artboards`, `create_node`, `export_artboard`) up
//! to the high-level design capabilities an automation client composes
//! a real design from: template / asset / theme libraries, the AI
//! themed-design generator, fill / text styling, content-aware magic
//! resize, and multi-format (svg/png/pdf) export.
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

/// Object-safe access to the live document graph and the high-level
/// design capabilities an automation client composes a design from.
///
/// `kcreate_mcp` deliberately depends only on `kcreate_core` (graph
/// types) and `kcreate_export` (SVG export); it must NOT depend on
/// `kcreate_bridge`. So anything that needs the renderer, the AI
/// generator, the template / asset / theme libraries, or PNG/PDF
/// export is expressed here as a trait method the bridge implements
/// over its process-global workspace. Open-ended inputs/outputs cross
/// this seam as `serde_json::Value` so the bridge owns the exact
/// payload shape (its `#[napi]`/report structs) without leaking those
/// types into this crate.
///
/// Every mutating method MUST go through the bridge's
/// `document::with_workspace_mut` op path so the result is undoable
/// and persisted — no fake/echo responses.
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

    /// List the ready-made templates available in the library,
    /// optionally narrowed by category slug and/or a search query.
    /// Returns a JSON array of `{id, name, category, ...}`.
    fn list_templates(&self, category: Option<&str>, query: Option<&str>) -> Result<Value, String>;

    /// Instantiate a template by id into the open document as a single
    /// undoable operation. Returns `{artboard_id, node_ids}`.
    fn apply_template(&self, template_id: Uuid) -> Result<Value, String>;

    /// Generate a themed multi-artboard design from a natural-language
    /// brief, reusing the on-device AI generator. `options_json` is a
    /// JSON object the bridge deserialises into its themed-design
    /// request (format / theme / sizing). Returns the apply result.
    fn generate_themed_design(&self, brief: &str, options_json: &str) -> Result<Value, String>;

    /// List the searchable, recolorable elements library, optionally
    /// narrowed by category and/or query. Returns a JSON array.
    fn list_assets(&self, category: Option<&str>, query: Option<&str>) -> Result<Value, String>;

    /// Insert a library asset by id under `parent_id` (or the active
    /// artboard when `None`) at `(x, y)`, optionally scaled so its
    /// longest side is `target_size` px. Single undoable op. Returns
    /// the inserted-asset descriptor.
    fn insert_asset(
        &self,
        asset_id: &str,
        parent_id: Option<Uuid>,
        x: f64,
        y: f64,
        target_size: Option<f64>,
    ) -> Result<Value, String>;

    /// Set a node's fill as a single undoable operation. `fill` is a
    /// `kcreate_core::node::FillStyle` JSON value.
    fn set_fill(&self, node_id: Uuid, fill: Value) -> Result<(), String>;

    /// Replace a text layer's content as a single undoable operation.
    fn set_text(&self, node_id: Uuid, content: &str) -> Result<(), String>;

    /// List the built-in professional themes. Returns a JSON array of
    /// `{id, name, ...}`.
    fn list_themes(&self) -> Result<Value, String>;

    /// Restyle the whole open document to the built-in theme with the
    /// given id, as a single undoable operation. Returns the apply
    /// report.
    fn apply_theme(&self, theme_id: &str) -> Result<Value, String>;

    /// Content-aware magic resize: reflow the design on `source_artboard_id`
    /// onto each of `targets` (a JSON array of resize-target specs) as a
    /// single undoable operation. Returns `{artboard_ids}`.
    fn magic_resize(&self, source_artboard_id: Uuid, targets: Value) -> Result<Value, String>;

    /// Export `node_ids` to `path` in `format` (`"svg"`, `"png"`, or
    /// `"pdf"`). `options` carries format-specific settings (e.g. PNG
    /// `width`/`height`/`scale`/`background`, PDF `widthMm`/`heightMm`).
    /// Returns `{path, format, bytes_written}`.
    fn export_design(
        &self,
        node_ids: &[Uuid],
        format: &str,
        path: &str,
        options: Value,
    ) -> Result<Value, String>;
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
    /// This tool supports `"svg"`. PNG/PDF support requires renderer
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
            format!("unsupported format: {other} (supported: svg)"),
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

// ---------------------------------------------------------------------------
// Shared param parsing helpers
// ---------------------------------------------------------------------------

/// Map a bridge-side error string to a JSON-RPC INTERNAL_ERROR pair.
fn internal(e: String) -> (i32, String) {
    (codes::INTERNAL_ERROR, e)
}

/// Deserialize required params, surfacing a clear INVALID_PARAMS error
/// (including when the client sent no `params` at all).
fn parse_required<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, (i32, String)> {
    serde_json::from_value(params)
        .map_err(|e| (codes::INVALID_PARAMS, format!("invalid params: {e}")))
}

/// Deserialize all-optional params, tolerating a missing/`null` body.
fn parse_optional<T: serde::de::DeserializeOwned + Default>(
    params: Value,
) -> Result<T, (i32, String)> {
    if params.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(params)
        .map_err(|e| (codes::INVALID_PARAMS, format!("invalid params: {e}")))
}

fn parse_uuid_arg(s: &str, field: &str) -> Result<Uuid, (i32, String)> {
    Uuid::parse_str(s).map_err(|e| (codes::INVALID_PARAMS, format!("bad {field}: {e}")))
}

fn opt_uuid_arg(s: Option<&str>, field: &str) -> Result<Option<Uuid>, (i32, String)> {
    match s {
        Some(s) if !s.is_empty() => Ok(Some(parse_uuid_arg(s, field)?)),
        _ => Ok(None),
    }
}

/// Convert a `#RRGGBB` / `#RRGGBBAA` hex string into a
/// `FillStyle::Solid` JSON value (channels normalised to 0..1) so an
/// agent can set a fill with a single human-friendly `color` argument.
fn hex_color_to_fill(hex: &str) -> Result<Value, (i32, String)> {
    let h = hex.trim().trim_start_matches('#');
    let bad = || {
        (
            codes::INVALID_PARAMS,
            format!("color must be #RRGGBB or #RRGGBBAA, got {hex:?}"),
        )
    };
    if !h.is_ascii() {
        return Err(bad());
    }
    let byte = |slice: &str| u8::from_str_radix(slice, 16).map_err(|_| bad());
    let (r, g, b, a) = match h.len() {
        6 => (byte(&h[0..2])?, byte(&h[2..4])?, byte(&h[4..6])?, 255u8),
        8 => (
            byte(&h[0..2])?,
            byte(&h[2..4])?,
            byte(&h[4..6])?,
            byte(&h[6..8])?,
        ),
        _ => return Err(bad()),
    };
    Ok(json!({
        "kind": "solid",
        "r": f32::from(r) / 255.0,
        "g": f32::from(g) / 255.0,
        "b": f32::from(b) / 255.0,
        "a": f32::from(a) / 255.0,
    }))
}

// ---------------------------------------------------------------------------
// New tool handlers (library / AI / styling / resize / export)
// ---------------------------------------------------------------------------

/// Params shared by `list_templates` and `list_assets`.
#[derive(Debug, Default, Deserialize)]
pub struct ListLibraryParams {
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
}

/// `list_templates`
pub fn handle_list_templates(
    access: &dyn DocumentAccess,
    params: Value,
) -> Result<Value, (i32, String)> {
    let p: ListLibraryParams = parse_optional(params)?;
    access
        .list_templates(p.category.as_deref(), p.query.as_deref())
        .map_err(internal)
}

/// `apply_template` params.
#[derive(Debug, Deserialize)]
pub struct ApplyTemplateParams {
    pub template_id: String,
}

/// `apply_template`
pub fn handle_apply_template(
    access: &dyn DocumentAccess,
    params: Value,
) -> Result<Value, (i32, String)> {
    let p: ApplyTemplateParams = parse_required(params)?;
    let id = parse_uuid_arg(&p.template_id, "template_id")?;
    access.apply_template(id).map_err(internal)
}

/// `generate_themed_design` params.
#[derive(Debug, Deserialize)]
pub struct GenerateThemedDesignParams {
    pub brief: String,
    /// Themed-design request object (format / theme / sizing). The
    /// bridge owns the exact schema; passed through verbatim.
    #[serde(default)]
    pub options: Value,
}

/// `generate_themed_design`
pub fn handle_generate_themed_design(
    access: &dyn DocumentAccess,
    params: Value,
) -> Result<Value, (i32, String)> {
    let p: GenerateThemedDesignParams = parse_required(params)?;
    if p.brief.trim().is_empty() {
        return Err((codes::INVALID_PARAMS, "brief must not be empty".into()));
    }
    let options_json = if p.options.is_null() {
        "{}".to_string()
    } else {
        p.options.to_string()
    };
    access
        .generate_themed_design(&p.brief, &options_json)
        .map_err(internal)
}

/// `list_assets`
pub fn handle_list_assets(
    access: &dyn DocumentAccess,
    params: Value,
) -> Result<Value, (i32, String)> {
    let p: ListLibraryParams = parse_optional(params)?;
    access
        .list_assets(p.category.as_deref(), p.query.as_deref())
        .map_err(internal)
}

/// `insert_asset` params.
#[derive(Debug, Deserialize)]
pub struct InsertAssetParams {
    pub asset_id: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default)]
    pub target_size: Option<f64>,
}

/// `insert_asset`
pub fn handle_insert_asset(
    access: &dyn DocumentAccess,
    params: Value,
) -> Result<Value, (i32, String)> {
    let p: InsertAssetParams = parse_required(params)?;
    let parent = opt_uuid_arg(p.parent_id.as_deref(), "parent_id")?;
    access
        .insert_asset(&p.asset_id, parent, p.x, p.y, p.target_size)
        .map_err(internal)
}

/// `set_fill` params.
#[derive(Debug, Deserialize)]
pub struct SetFillParams {
    pub node_id: String,
    /// A full `FillStyle` JSON value (`{"kind":"solid","r":..}` etc.).
    #[serde(default)]
    pub fill: Option<Value>,
    /// Convenience alternative to `fill`: a `#RRGGBB`/`#RRGGBBAA` hex.
    #[serde(default)]
    pub color: Option<String>,
}

/// `set_fill`
pub fn handle_set_fill(access: &dyn DocumentAccess, params: Value) -> Result<Value, (i32, String)> {
    let p: SetFillParams = parse_required(params)?;
    let node_id = parse_uuid_arg(&p.node_id, "node_id")?;
    let fill = match (p.fill, p.color) {
        (Some(f), _) => f,
        (None, Some(c)) => hex_color_to_fill(&c)?,
        (None, None) => {
            return Err((
                codes::INVALID_PARAMS,
                "set_fill requires `fill` (FillStyle JSON) or `color` (hex string)".into(),
            ))
        }
    };
    access.set_fill(node_id, fill).map_err(internal)?;
    Ok(json!({ "node_id": node_id.to_string(), "ok": true }))
}

/// `set_text` params.
#[derive(Debug, Deserialize)]
pub struct SetTextParams {
    pub node_id: String,
    pub content: String,
}

/// `set_text`
pub fn handle_set_text(access: &dyn DocumentAccess, params: Value) -> Result<Value, (i32, String)> {
    let p: SetTextParams = parse_required(params)?;
    let node_id = parse_uuid_arg(&p.node_id, "node_id")?;
    access.set_text(node_id, &p.content).map_err(internal)?;
    Ok(json!({ "node_id": node_id.to_string(), "ok": true }))
}

/// `list_themes`
pub fn handle_list_themes(access: &dyn DocumentAccess) -> Result<Value, (i32, String)> {
    access.list_themes().map_err(internal)
}

/// `apply_theme` params.
#[derive(Debug, Deserialize)]
pub struct ApplyThemeParams {
    pub theme_id: String,
}

/// `apply_theme`
pub fn handle_apply_theme(
    access: &dyn DocumentAccess,
    params: Value,
) -> Result<Value, (i32, String)> {
    let p: ApplyThemeParams = parse_required(params)?;
    if p.theme_id.trim().is_empty() {
        return Err((codes::INVALID_PARAMS, "theme_id must not be empty".into()));
    }
    access.apply_theme(&p.theme_id).map_err(internal)
}

/// `magic_resize` params.
#[derive(Debug, Deserialize)]
pub struct MagicResizeParams {
    pub source_artboard_id: String,
    /// JSON array of resize-target specs (`{preset}` or `{width,height}`).
    pub targets: Value,
}

/// `magic_resize`
pub fn handle_magic_resize(
    access: &dyn DocumentAccess,
    params: Value,
) -> Result<Value, (i32, String)> {
    let p: MagicResizeParams = parse_required(params)?;
    let src = parse_uuid_arg(&p.source_artboard_id, "source_artboard_id")?;
    if !p.targets.is_array() {
        return Err((
            codes::INVALID_PARAMS,
            "targets must be a JSON array of resize-target specs".into(),
        ));
    }
    access.magic_resize(src, p.targets).map_err(internal)
}

/// `export_design` params.
#[derive(Debug, Deserialize)]
pub struct ExportDesignParams {
    #[serde(default)]
    pub node_id: Option<String>,
    #[serde(default)]
    pub node_ids: Option<Vec<String>>,
    pub format: String,
    pub path: String,
    #[serde(default)]
    pub options: Value,
}

/// `export_design`
pub fn handle_export_design(
    access: &dyn DocumentAccess,
    params: Value,
) -> Result<Value, (i32, String)> {
    let p: ExportDesignParams = parse_required(params)?;
    let mut ids: Vec<Uuid> = Vec::new();
    if let Some(one) = p.node_id.as_deref().filter(|s| !s.is_empty()) {
        ids.push(parse_uuid_arg(one, "node_id")?);
    }
    if let Some(list) = p.node_ids {
        for s in list.iter().filter(|s| !s.is_empty()) {
            ids.push(parse_uuid_arg(s, "node_ids")?);
        }
    }
    access
        .export_design(&ids, &p.format, &p.path, p.options)
        .map_err(internal)
}

// ---------------------------------------------------------------------------
// Tool registry — single source of truth for discovery + dispatch
// ---------------------------------------------------------------------------

/// One advertised tool: name, human description, and JSON-Schema for
/// its arguments. Serialised verbatim into the MCP `tools/list`
/// response (`inputSchema` is the MCP-standard field name).
#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// Every callable tool name, including the back-compat `export_artboard`
/// alias. Used by the server to decide whether a *direct* JSON-RPC
/// method name is a (permission-gated) tool call.
pub const TOOL_NAMES: &[&str] = &[
    "list_artboards",
    "create_node",
    "export_artboard",
    "list_templates",
    "apply_template",
    "generate_themed_design",
    "list_assets",
    "insert_asset",
    "set_fill",
    "set_text",
    "list_themes",
    "apply_theme",
    "magic_resize",
    "export_design",
];

/// Whether `name` is a permission-gated tool (vs. a protocol method
/// like `initialize` / `tools/list`).
#[must_use]
pub fn is_tool(name: &str) -> bool {
    TOOL_NAMES.contains(&name)
}

fn obj_schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": true,
    })
}

/// The advertised tool catalogue for `tools/list`. The back-compat
/// `export_artboard` alias is intentionally not advertised — agents
/// should discover the richer `export_design` (svg/png/pdf) instead.
#[must_use]
pub fn tool_specs() -> Vec<ToolSpec> {
    let spec = |name: &str, description: &str, schema: Value| ToolSpec {
        name: name.to_string(),
        description: description.to_string(),
        input_schema: schema,
    };
    vec![
        spec(
            "list_artboards",
            "List every artboard in the open document with its id, name and bounds.",
            obj_schema(json!({}), &[]),
        ),
        spec(
            "create_node",
            "Create a node (artboard/group/vector/text/...) and return its id.",
            obj_schema(
                json!({
                    "node_type": {"type": "string", "description": "page|artboard|group|vector|raster|text|component|layout"},
                    "name": {"type": "string"},
                    "parent_id": {"type": "string", "description": "Optional parent node id (uuid)."}
                }),
                &["node_type", "name"],
            ),
        ),
        spec(
            "list_templates",
            "List ready-made templates, optionally filtered by category and/or search query.",
            obj_schema(
                json!({
                    "category": {"type": "string"},
                    "query": {"type": "string"}
                }),
                &[],
            ),
        ),
        spec(
            "apply_template",
            "Instantiate a template by id into the open document (undoable).",
            obj_schema(
                json!({"template_id": {"type": "string"}}),
                &["template_id"],
            ),
        ),
        spec(
            "generate_themed_design",
            "Generate a themed multi-artboard design from a natural-language brief using the on-device AI generator.",
            obj_schema(
                json!({
                    "brief": {"type": "string"},
                    "options": {"type": "object", "description": "Optional themed-design request (format/themeId/sizing)."}
                }),
                &["brief"],
            ),
        ),
        spec(
            "list_assets",
            "List the searchable elements library, optionally filtered by category and/or query.",
            obj_schema(
                json!({
                    "category": {"type": "string"},
                    "query": {"type": "string"}
                }),
                &[],
            ),
        ),
        spec(
            "insert_asset",
            "Insert a library asset by id at (x,y) under a parent, optionally scaled to target_size (undoable).",
            obj_schema(
                json!({
                    "asset_id": {"type": "string"},
                    "parent_id": {"type": "string"},
                    "x": {"type": "number"},
                    "y": {"type": "number"},
                    "target_size": {"type": "number", "description": "Scale so the longest side is this many px."}
                }),
                &["asset_id"],
            ),
        ),
        spec(
            "set_fill",
            "Set a node's fill (undoable). Provide `color` (#RRGGBB[AA]) or a full `fill` FillStyle object.",
            obj_schema(
                json!({
                    "node_id": {"type": "string"},
                    "color": {"type": "string", "description": "#RRGGBB or #RRGGBBAA"},
                    "fill": {"type": "object", "description": "Full FillStyle JSON (alternative to color)."}
                }),
                &["node_id"],
            ),
        ),
        spec(
            "set_text",
            "Replace a text layer's content (undoable).",
            obj_schema(
                json!({
                    "node_id": {"type": "string"},
                    "content": {"type": "string"}
                }),
                &["node_id", "content"],
            ),
        ),
        spec(
            "list_themes",
            "List the built-in professional themes with their ids and names.",
            obj_schema(json!({}), &[]),
        ),
        spec(
            "apply_theme",
            "Restyle the whole open document to a built-in theme by id (undoable).",
            obj_schema(json!({"theme_id": {"type": "string"}}), &["theme_id"]),
        ),
        spec(
            "magic_resize",
            "Content-aware reflow of a source artboard onto one or more target sizes (undoable).",
            obj_schema(
                json!({
                    "source_artboard_id": {"type": "string"},
                    "targets": {
                        "type": "array",
                        "items": {"type": "object", "properties": {
                            "preset": {"type": "string"},
                            "width": {"type": "number"},
                            "height": {"type": "number"},
                            "name": {"type": "string"}
                        }}
                    }
                }),
                &["source_artboard_id", "targets"],
            ),
        ),
        spec(
            "export_design",
            "Export node(s) to a file as svg, png or pdf. Returns {path, format, bytes_written}.",
            obj_schema(
                json!({
                    "node_id": {"type": "string"},
                    "node_ids": {"type": "array", "items": {"type": "string"}},
                    "format": {"type": "string", "description": "svg | png | pdf"},
                    "path": {"type": "string"},
                    "options": {"type": "object", "description": "Format-specific export options. PNG keys: width, height, scale, background. PDF keys (camelCase): widthMm, heightMm, colorMode, cmykDither. SVG honours node_id(s) and needs no options."}
                }),
                &["format", "path"],
            ),
        ),
    ]
}

/// Route a tool call (by name) to its handler. Shared by the server's
/// `tools/call` path and its back-compat direct-method dispatch so both
/// resolve against one source of truth. Permission gating happens in
/// the server *before* this is called.
pub fn dispatch_tool(
    access: &dyn DocumentAccess,
    name: &str,
    params: Value,
) -> Result<Value, (i32, String)> {
    match name {
        "list_artboards" => handle_list_artboards(access),
        "create_node" => handle_create_node(access, params),
        "export_artboard" => handle_export_artboard(access, params),
        "list_templates" => handle_list_templates(access, params),
        "apply_template" => handle_apply_template(access, params),
        "generate_themed_design" => handle_generate_themed_design(access, params),
        "list_assets" => handle_list_assets(access, params),
        "insert_asset" => handle_insert_asset(access, params),
        "set_fill" => handle_set_fill(access, params),
        "set_text" => handle_set_text(access, params),
        "list_themes" => handle_list_themes(access),
        "apply_theme" => handle_apply_theme(access, params),
        "magic_resize" => handle_magic_resize(access, params),
        "export_design" => handle_export_design(access, params),
        other => Err((codes::METHOD_NOT_FOUND, format!("unknown tool: {other}"))),
    }
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

        // The high-level library / AI / renderer capabilities are owned
        // by the bridge (over the real workspace) and are exercised by
        // the cross-crate integration test in `kcreate_tests`. This
        // in-memory double implements the graph-level methods it can and
        // returns an explicit "not supported" error for the rest so unit
        // tests here stay focused on the protocol/validation layer.
        fn list_templates(
            &self,
            _category: Option<&str>,
            _query: Option<&str>,
        ) -> Result<Value, String> {
            Err("template library not available in the in-memory test double".into())
        }
        fn apply_template(&self, _template_id: Uuid) -> Result<Value, String> {
            Err("template library not available in the in-memory test double".into())
        }
        fn generate_themed_design(
            &self,
            _brief: &str,
            _options_json: &str,
        ) -> Result<Value, String> {
            Err("AI generator not available in the in-memory test double".into())
        }
        fn list_assets(
            &self,
            _category: Option<&str>,
            _query: Option<&str>,
        ) -> Result<Value, String> {
            Err("asset library not available in the in-memory test double".into())
        }
        fn insert_asset(
            &self,
            _asset_id: &str,
            _parent_id: Option<Uuid>,
            _x: f64,
            _y: f64,
            _target_size: Option<f64>,
        ) -> Result<Value, String> {
            Err("asset library not available in the in-memory test double".into())
        }
        fn set_fill(&self, node_id: Uuid, fill: Value) -> Result<(), String> {
            let parsed: kcreate_core::node::FillStyle =
                serde_json::from_value(fill).map_err(|e| format!("bad fill: {e}"))?;
            let mut guard = self.0.lock();
            let node = guard
                .get_node_mut(node_id)
                .ok_or_else(|| format!("node not found: {node_id}"))?;
            node.style.fill = parsed;
            Ok(())
        }
        fn set_text(&self, node_id: Uuid, content: &str) -> Result<(), String> {
            let mut guard = self.0.lock();
            let node = guard
                .get_node_mut(node_id)
                .ok_or_else(|| format!("node not found: {node_id}"))?;
            node.metadata
                .insert("text".to_string(), json!({ "text": content }));
            Ok(())
        }
        fn list_themes(&self) -> Result<Value, String> {
            Err("theme library not available in the in-memory test double".into())
        }
        fn apply_theme(&self, _theme_id: &str) -> Result<Value, String> {
            Err("theme library not available in the in-memory test double".into())
        }
        fn magic_resize(&self, _source: Uuid, _targets: Value) -> Result<Value, String> {
            Err("magic resize not available in the in-memory test double".into())
        }
        fn export_design(
            &self,
            node_ids: &[Uuid],
            format: &str,
            path: &str,
            _options: Value,
        ) -> Result<Value, String> {
            match format {
                "svg" => {
                    let svg = self.export_svg(node_ids)?;
                    std::fs::write(path, &svg).map_err(|e| format!("write failed: {e}"))?;
                    Ok(json!({ "path": path, "format": "svg", "bytes_written": svg.len() }))
                }
                other => Err(format!(
                    "the in-memory test double only exports svg, got {other}"
                )),
            }
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

    #[test]
    fn hex_color_to_fill_parses_rgb_and_rgba() {
        let solid = hex_color_to_fill("#7C3AED").expect("rgb");
        assert_eq!(solid["kind"], "solid");
        assert!((solid["r"].as_f64().unwrap() - 124.0 / 255.0).abs() < 1e-6);
        assert!((solid["a"].as_f64().unwrap() - 1.0).abs() < 1e-6);
        let rgba = hex_color_to_fill("#00000080").expect("rgba");
        assert!((rgba["a"].as_f64().unwrap() - 128.0 / 255.0).abs() < 1e-6);
        assert!(hex_color_to_fill("nope").is_err());
        assert!(hex_color_to_fill("#fff").is_err());
    }

    #[test]
    fn set_fill_via_color_mutates_node() {
        let mut doc = DocumentGraph::new();
        let node_id = doc
            .insert_node(Node::new(NodeType::VectorLayer, "shape"))
            .expect("insert");
        let access: Arc<dyn DocumentAccess> = Arc::new(InMemoryDoc(Mutex::new(doc)));
        let out = handle_set_fill(
            &*access,
            json!({ "node_id": node_id.to_string(), "color": "#112233" }),
        )
        .expect("ok");
        assert_eq!(out["ok"], true);
        // Missing both fill and color is a clear param error.
        let err = handle_set_fill(&*access, json!({ "node_id": node_id.to_string() }))
            .expect_err("needs fill or color");
        assert_eq!(err.0, codes::INVALID_PARAMS);
    }

    #[test]
    fn dispatch_tool_routes_and_rejects_unknown() {
        let doc = DocumentGraph::new();
        let access: Arc<dyn DocumentAccess> = Arc::new(InMemoryDoc(Mutex::new(doc)));
        let ok = dispatch_tool(
            &*access,
            "create_node",
            json!({"node_type": "artboard", "name": "A"}),
        )
        .expect("ok");
        assert!(ok["id"].as_str().is_some());
        let err = dispatch_tool(&*access, "does_not_exist", Value::Null).expect_err("unknown");
        assert_eq!(err.0, codes::METHOD_NOT_FOUND);
    }

    #[test]
    fn tool_specs_cover_registry_and_are_well_formed() {
        let specs = tool_specs();
        // Every advertised tool resolves as a tool name.
        for s in &specs {
            assert!(is_tool(&s.name), "{} should be a known tool", s.name);
            assert_eq!(s.input_schema["type"], "object");
        }
        // The advertised set is the modern catalogue (everything except
        // the hidden back-compat `export_artboard` alias).
        assert_eq!(specs.len(), TOOL_NAMES.len() - 1);
        assert!(specs.iter().all(|s| s.name != "export_artboard"));
        assert!(is_tool("export_artboard"));
    }
}

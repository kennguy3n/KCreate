//! Project model — the top-level container that ties together the
//! document graph, operation log, brand kits, design tokens, and
//! export presets.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::document::{DocumentError, DocumentGraph};
use crate::node::{Bounds, Node, NodeType, RgbaColor};
use crate::operation::{Operation, OperationLog};

/// Errors from project-level operations.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProjectError {
    #[error("document error: {0}")]
    Document(#[from] DocumentError),
    #[error("brand kit {0} not found")]
    BrandKitNotFound(Uuid),
    #[error("export preset {0} not found")]
    ExportPresetNotFound(Uuid),
}

/// Reusable typography token (font + size + line height + tracking).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypographyToken {
    pub font_family: String,
    pub font_weight: u16,
    pub font_size: f32,
    pub line_height: f32,
    pub letter_spacing: f32,
}

/// A drop-shadow design token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)] // contains f32 fields
pub struct ShadowToken {
    pub offset_x: f32,
    pub offset_y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: RgbaColor,
}

/// Project-wide reusable tokens.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DesignTokens {
    pub colors: HashMap<String, RgbaColor>,
    pub typography: HashMap<String, TypographyToken>,
    pub spacing: HashMap<String, f32>,
    pub radii: HashMap<String, f32>,
    pub shadows: HashMap<String, ShadowToken>,
}

/// A named color reference inside a brand kit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedColor {
    pub name: String,
    pub color: RgbaColor,
}

/// A font reference inside a brand kit. `path` is set when the font is
/// embedded as an asset; otherwise we look it up by family name on the
/// host system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontRef {
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    pub embedded_asset_id: Option<Uuid>,
}

/// Supported export targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Png,
    Svg,
    Pdf,
    Webp,
    Jpeg,
}

/// A pre-configured export target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportPreset {
    pub id: Uuid,
    pub name: String,
    pub format: ExportFormat,
    pub scale: f32,
    pub suffix: String,
}

impl ExportPreset {
    #[must_use]
    pub fn new(name: impl Into<String>, format: ExportFormat, scale: f32) -> Self {
        let suffix = match format {
            ExportFormat::Png => ".png".to_string(),
            ExportFormat::Svg => ".svg".to_string(),
            ExportFormat::Pdf => ".pdf".to_string(),
            ExportFormat::Webp => ".webp".to_string(),
            ExportFormat::Jpeg => ".jpg".to_string(),
        };
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            format,
            scale,
            suffix,
        }
    }
}

/// A brand kit: top-level set of palette / typography / logos / spacing
/// / export rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrandKit {
    pub id: Uuid,
    pub name: String,
    pub logo_asset_id: Option<Uuid>,
    pub colors: Vec<NamedColor>,
    pub fonts: Vec<FontRef>,
    pub spacing_scale: Vec<f32>,
    pub export_rules: Vec<ExportPreset>,
}

impl BrandKit {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            logo_asset_id: None,
            colors: Vec::new(),
            fonts: Vec::new(),
            spacing_scale: Vec::new(),
            export_rules: Vec::new(),
        }
    }
}

/// The top-level project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    pub document: DocumentGraph,
    pub operation_log: OperationLog,
    pub design_tokens: DesignTokens,
    pub brand_kits: Vec<BrandKit>,
    pub export_presets: Vec<ExportPreset>,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
}

impl Project {
    /// Create a new, empty project with sensible defaults.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            document: DocumentGraph::new(),
            operation_log: OperationLog::default(),
            design_tokens: DesignTokens::default(),
            brand_kits: Vec::new(),
            export_presets: default_export_presets(),
            created_at: now,
            modified_at: now,
        }
    }

    /// Add a new page (with one artboard child) and return the page id.
    pub fn add_page(&mut self, name: impl Into<String>) -> Result<Uuid, ProjectError> {
        let name = name.into();
        let mut page = Node::new(NodeType::Page, name.clone());
        page.bounds = Bounds::new(0.0, 0.0, 1920.0, 1080.0);
        let page_id = page.id;
        self.document.insert_node(page)?;

        let mut artboard = Node::new(NodeType::Artboard, format!("{name} / Artboard 1"));
        artboard.parent_id = Some(page_id);
        artboard.bounds = Bounds::new(0.0, 0.0, 1920.0, 1080.0);
        self.document.insert_node(artboard)?;

        self.touch_modified();
        Ok(page_id)
    }

    /// Append `operation` to the log and bump the modified timestamp.
    pub fn execute_operation(&mut self, operation: Operation) {
        self.operation_log.push(operation);
        self.touch_modified();
    }

    /// Roll back the most recent operation. Returns the rolled-back
    /// operation, or `None` if there is nothing to undo. **Note:** this
    /// method moves the cursor in the log; it does not patch the
    /// document graph itself — the caller is responsible for applying
    /// `before_patch` to its in-memory state. We split that step out
    /// because patches are application-defined.
    pub fn undo(&mut self) -> Option<Operation> {
        let op = self.operation_log.undo()?.clone();
        self.touch_modified();
        Some(op)
    }

    /// Re-apply the next operation. See [`Self::undo`] for the
    /// responsibilities split.
    pub fn redo(&mut self) -> Option<Operation> {
        let op = self.operation_log.redo()?.clone();
        self.touch_modified();
        Some(op)
    }

    /// Add or replace a brand kit. Replaces by `id`.
    pub fn upsert_brand_kit(&mut self, kit: BrandKit) {
        if let Some(slot) = self.brand_kits.iter_mut().find(|k| k.id == kit.id) {
            *slot = kit;
        } else {
            self.brand_kits.push(kit);
        }
        self.touch_modified();
    }

    /// Look up a brand kit by id.
    pub fn brand_kit(&self, id: Uuid) -> Result<&BrandKit, ProjectError> {
        self.brand_kits
            .iter()
            .find(|k| k.id == id)
            .ok_or(ProjectError::BrandKitNotFound(id))
    }

    /// Look up an export preset by id.
    pub fn export_preset(&self, id: Uuid) -> Result<&ExportPreset, ProjectError> {
        self.export_presets
            .iter()
            .find(|p| p.id == id)
            .ok_or(ProjectError::ExportPresetNotFound(id))
    }

    fn touch_modified(&mut self) {
        self.modified_at = Utc::now();
    }
}

/// Built-in export presets every new project starts with.
fn default_export_presets() -> Vec<ExportPreset> {
    vec![
        ExportPreset::new("PNG @1x", ExportFormat::Png, 1.0),
        ExportPreset::new("PNG @2x", ExportFormat::Png, 2.0),
        ExportPreset::new("PNG @3x", ExportFormat::Png, 3.0),
        ExportPreset::new("SVG", ExportFormat::Svg, 1.0),
        ExportPreset::new("PDF print", ExportFormat::Pdf, 1.0),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn new_project_has_defaults() {
        let p = Project::new("My Project");
        assert_eq!(p.name, "My Project");
        assert_eq!(p.document.node_count(), 0);
        assert!(p.operation_log.is_empty());
        assert!(!p.export_presets.is_empty());
        assert!(p.brand_kits.is_empty());
    }

    #[test]
    fn add_page_creates_page_and_artboard() {
        let mut p = Project::new("My Project");
        let page_id = p.add_page("Home").expect("page");
        assert_eq!(p.document.node_count(), 2);
        let page = p.document.get_node(page_id).expect("page");
        assert_eq!(page.node_type, NodeType::Page);
        let kids = p.document.children_of(page_id);
        assert_eq!(kids.len(), 1);
        let art = p.document.get_node(kids[0]).expect("artboard");
        assert_eq!(art.node_type, NodeType::Artboard);
    }

    #[test]
    fn execute_undo_redo_round_trip() {
        let mut p = Project::new("My Project");
        let op = Operation::new("user", "noop", json!({}), json!({}), Vec::new());
        p.execute_operation(op);
        assert!(p.operation_log.can_undo());
        let _ = p.undo().expect("undo");
        assert!(p.operation_log.can_redo());
        let _ = p.redo().expect("redo");
        assert!(!p.operation_log.can_redo());
    }

    #[test]
    fn upsert_brand_kit_replaces_in_place() {
        let mut p = Project::new("p");
        let mut k = BrandKit::new("Default");
        let kid = k.id;
        p.upsert_brand_kit(k.clone());
        k.name = "Renamed".to_string();
        p.upsert_brand_kit(k);
        assert_eq!(p.brand_kits.len(), 1);
        assert_eq!(p.brand_kit(kid).expect("kit").name, "Renamed");
    }

    #[test]
    fn export_preset_lookup() {
        let p = Project::new("p");
        let preset = p.export_presets[0].clone();
        let same = p.export_preset(preset.id).expect("found");
        assert_eq!(same.id, preset.id);
        let missing = p.export_preset(Uuid::new_v4());
        assert!(matches!(
            missing,
            Err(ProjectError::ExportPresetNotFound(_))
        ));
    }

    #[test]
    fn project_serialize_roundtrip() {
        let mut p = Project::new("p");
        p.add_page("Home").expect("page");
        let s = serde_json::to_string(&p).expect("serialize");
        let p2: Project = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(p.id, p2.id);
        assert_eq!(p.document.node_count(), p2.document.node_count());
        assert_eq!(p.export_presets.len(), p2.export_presets.len());
    }

    #[test]
    fn brand_kit_not_found_errors() {
        let p = Project::new("p");
        let err = p.brand_kit(Uuid::new_v4()).expect_err("missing");
        assert!(matches!(err, ProjectError::BrandKitNotFound(_)));
    }
}

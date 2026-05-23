//! Project model — the top-level container that ties together the
//! document graph, operation log, brand kits, design tokens, and
//! export presets.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::color::ColorSettings;
use crate::component::{ComponentDefinition, ComponentError};
use crate::document::{DocumentError, DocumentGraph};
use crate::node::{Bounds, Node, NodeType, RgbaColor};
use crate::operation::{Operation, OperationLog};

/// Errors from project-level operations.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProjectError {
    #[error("document error: {0}")]
    Document(#[from] DocumentError),
    #[error("component error: {0}")]
    Component(#[from] ComponentError),
    #[error("brand kit {0} not found")]
    BrandKitNotFound(Uuid),
    #[error("export preset {0} not found")]
    ExportPresetNotFound(Uuid),
    #[error("component {0} not found")]
    ComponentNotFound(Uuid),
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
    /// Reusable component definitions registered with the project.
    /// Stored as a map so lookups by id are O(1) and the wire format
    /// is stable across re-saves (no ordering churn).
    #[serde(default)]
    pub components: HashMap<Uuid, ComponentDefinition>,
    /// Document-level color management settings (RGB / CMYK working
    /// spaces, rendering intent, soft-proof). `#[serde(default)]` so
    /// older project files still deserialize cleanly with sRGB
    /// defaults.
    #[serde(default)]
    pub color_settings: ColorSettings,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
}

impl Project {
    /// Create a new, empty project with the default 256-deep undo log.
    ///
    /// **Production callers that have a `RuntimeConfig` should prefer
    /// [`Self::with_max_undo_depth`]** so the log respects the
    /// device-tier budget computed by
    /// [`crate::config::DeviceTier::default_undo_depth`] (32 on Tier 0,
    /// 128 on Tier 1, 256 on Tier 2, 1024 on Tier 3). This entry
    /// point keeps the simple `Project::new(name)` ergonomics for
    /// tests, examples, and any caller for whom the default budget is
    /// the right answer.
    ///
    /// Note: `export_presets`, `design_tokens`, and `brand_kits` start
    /// empty so identifiers are stable across save/reopen. Call
    /// [`Self::install_default_export_presets`] explicitly when
    /// creating a brand-new project (typically from the home page);
    /// callers reopening an existing project must rely on persistence
    /// to restore them, not on auto-population that would change ids.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        // 256 mirrors `OperationLog::default()` (Tier 2). Production
        // paths override via `with_max_undo_depth`.
        Self::with_max_undo_depth(name, 256)
    }

    /// Create a new, empty project whose undo log retains at most
    /// `max_undo_depth` operations.
    ///
    /// This is the constructor production paths should use: it lets
    /// the bridge thread the device-tier budget from
    /// [`crate::config::RuntimeConfig::max_undo_depth`] all the way
    /// down to the `OperationLog`, so a Tier 0 device (< 8 GB RAM)
    /// actually gets a 32-deep history instead of silently retaining
    /// 256 ops and exceeding its memory budget, and a Tier 3 device
    /// (≥ 32 GB) actually gets the 1024-deep history its docstring
    /// promises instead of being capped at 256.
    ///
    /// `0` is clamped to `1` by [`OperationLog::new`] — a useless but
    /// well-defined edge.
    #[must_use]
    pub fn with_max_undo_depth(name: impl Into<String>, max_undo_depth: usize) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            document: DocumentGraph::new(),
            operation_log: OperationLog::new(max_undo_depth),
            design_tokens: DesignTokens::default(),
            brand_kits: Vec::new(),
            export_presets: Vec::new(),
            components: HashMap::new(),
            color_settings: ColorSettings::default(),
            created_at: now,
            modified_at: now,
        }
    }

    /// Install the standard PNG/SVG/PDF export presets. Intended for
    /// freshly-created projects; do NOT call this after
    /// [`Self::new`] when re-opening an existing project, or you will
    /// shadow whatever the user persisted.
    pub fn install_default_export_presets(&mut self) {
        self.export_presets = default_export_presets();
        self.touch_modified();
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

    /// Create a master page (template) with the given layout.
    ///
    /// Returns the new page node id. Master pages are flagged via
    /// [`crate::node::MASTER_PAGE_METADATA_KEY`] and excluded from
    /// the normal page navigator by callers that filter on that flag.
    ///
    /// The master page is created with no children — callers add
    /// header / footer / page-number layers themselves before applying
    /// it to content pages.
    pub fn create_master_page(
        &mut self,
        name: impl Into<String>,
        layout: crate::node::PageLayout,
    ) -> Result<Uuid, ProjectError> {
        let mut page = Node::new(NodeType::Page, name);
        let (w_mm, h_mm) = layout.dimensions_mm();
        // 1 mm = ~3.7795275591 px at 96 dpi; bounds are stored in
        // document-space pixels so the canvas can render the page at
        // its natural size.
        let px_per_mm = 96.0 / 25.4;
        page.bounds = Bounds::new(0.0, 0.0, w_mm * px_per_mm, h_mm * px_per_mm);
        page.set_page_layout(&layout);
        page.set_master_page(true);
        let id = page.id;
        self.document.insert_node(page)?;
        self.touch_modified();
        Ok(id)
    }

    /// Iterate all pages flagged as master pages, sorted by name for
    /// deterministic UI.
    #[must_use]
    pub fn list_master_pages(&self) -> Vec<&Node> {
        let mut out: Vec<&Node> = self
            .document
            .iter()
            .map(|(_, n)| n)
            .filter(|n| n.is_master_page())
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Attach `master_page_id` to the content page's layout.
    ///
    /// Returns an error if either id is missing, the target is not a
    /// `Page`, or `master_page_id` is not flagged as a master.
    pub fn apply_master_page(
        &mut self,
        content_page_id: Uuid,
        master_page_id: Uuid,
    ) -> Result<(), ProjectError> {
        // Validate master is real and is_master.
        let master = self
            .document
            .get_node(master_page_id)
            .ok_or(DocumentError::NodeNotFound(master_page_id))?;
        if !master.is_master_page() {
            return Err(DocumentError::WrongNodeType {
                id: master_page_id,
                expected: NodeType::Page,
                got: master.node_type,
            }
            .into());
        }
        let target = self
            .document
            .get_node_mut(content_page_id)
            .ok_or(DocumentError::NodeNotFound(content_page_id))?;
        if target.node_type != NodeType::Page {
            return Err(DocumentError::WrongNodeType {
                id: content_page_id,
                expected: NodeType::Page,
                got: target.node_type,
            }
            .into());
        }
        let mut layout = target.page_layout().unwrap_or_default();
        layout.master_page_id = Some(master_page_id);
        target.set_page_layout(&layout);
        self.touch_modified();
        Ok(())
    }

    /// Clear the master page reference on a content page. No-op when
    /// the page has no layout or no master attached.
    pub fn detach_master_page(&mut self, content_page_id: Uuid) -> Result<(), ProjectError> {
        let page = self
            .document
            .get_node_mut(content_page_id)
            .ok_or(DocumentError::NodeNotFound(content_page_id))?;
        if page.node_type != NodeType::Page {
            return Err(DocumentError::WrongNodeType {
                id: content_page_id,
                expected: NodeType::Page,
                got: page.node_type,
            }
            .into());
        }
        if let Some(mut layout) = page.page_layout() {
            layout.master_page_id = None;
            page.set_page_layout(&layout);
            self.touch_modified();
        }
        Ok(())
    }

    /// Resolve the master page id (if any) attached to `page_id`.
    #[must_use]
    pub fn resolve_master_page(&self, page_id: Uuid) -> Option<Uuid> {
        self.document
            .get_node(page_id)
            .and_then(Node::page_layout)
            .and_then(|l| l.master_page_id)
    }

    /// Append `operation` to the log and bump the modified timestamp.
    pub fn execute_operation(&mut self, operation: Operation) {
        self.operation_log.push(operation);
        self.touch_modified();
    }

    /// Roll back the most recent operation. Returns the rolled-back
    /// operation, or `None` if there is nothing to undo.
    ///
    /// # Contract: host-driven patch application
    ///
    /// This method **only moves the cursor in the operation log**; it
    /// deliberately does not touch the `DocumentGraph`. The caller is
    /// responsible for applying `Operation::before_patch` to its
    /// in-memory state to actually revert the change.
    ///
    /// Why the split (this is intentional architecture, not a stub):
    ///
    /// 1. Patches are application-defined. The host UI groups
    ///    multiple bridge calls into a single user-facing operation
    ///    (e.g. a drag = one `move_node` op, not one op per pointer
    ///    sample). Auto-patching here would force a one-bridge-call =
    ///    one-op model that the editor's gesture layer does not want.
    /// 2. Replay paths exist that **must not** record new ops while
    ///    applying patches (otherwise replay would double the log).
    ///    Keeping the cursor move and the graph patch as separate
    ///    steps gives those paths explicit control.
    /// 3. The cost of forgetting to apply is a visual glitch, not a
    ///    corruption — the graph and log remain internally consistent.
    ///
    /// The pairing on the bridge side is:
    /// `document_create_node` / `update_node` / `delete_node` mutate
    /// the graph directly, `document_record_operation` appends to the
    /// log, and `document_undo` / `redo` only move the log cursor.
    pub fn undo(&mut self) -> Option<Operation> {
        let op = self.operation_log.undo()?.clone();
        self.touch_modified();
        Some(op)
    }

    /// Re-apply the next operation in the log. Returns the operation
    /// to be replayed, or `None` if the redo stack is empty.
    ///
    /// Mirrors [`Self::undo`]: callers apply `Operation::after_patch`
    /// to their in-memory state. See `undo`'s docstring for the full
    /// rationale on why patch application is host-driven.
    pub fn redo(&mut self) -> Option<Operation> {
        let op = self.operation_log.redo()?.clone();
        self.touch_modified();
        Some(op)
    }

    /// Peek at the operation that the next [`Self::undo`] would return,
    /// without moving the log cursor and without touching
    /// `modified_at`. Returns a cloned [`Operation`] so the borrow on
    /// `self.operation_log` is released immediately; callers typically
    /// use this to apply `before_patch` against the live state and only
    /// commit the cursor move via [`Self::undo`] on success.
    #[must_use]
    pub fn pending_undo(&self) -> Option<Operation> {
        self.operation_log.peek_undo().cloned()
    }

    /// Peek at the operation that the next [`Self::redo`] would return,
    /// without moving the log cursor and without touching
    /// `modified_at`. See [`Self::pending_undo`] for the atomicity
    /// rationale.
    #[must_use]
    pub fn pending_redo(&self) -> Option<Operation> {
        self.operation_log.peek_redo().cloned()
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

    /// Register a component definition. Returns the definition's id.
    /// If a component with the same id already exists, it is
    /// replaced (`HashMap::insert` semantics) — this is how the
    /// bridge upserts edits made from the UI.
    pub fn register_component(&mut self, definition: ComponentDefinition) -> Uuid {
        let id = definition.id;
        self.components.insert(id, definition);
        self.touch_modified();
        id
    }

    /// Look up a component by id.
    #[must_use]
    pub fn get_component(&self, id: Uuid) -> Option<&ComponentDefinition> {
        self.components.get(&id)
    }

    /// Mutable look-up — bridge mutates variants and properties
    /// through this handle.
    pub fn get_component_mut(&mut self, id: Uuid) -> Option<&mut ComponentDefinition> {
        self.components.get_mut(&id)
    }

    /// All registered components, sorted by name. The sort is for UI
    /// determinism (component list) and is not part of the
    /// persistence contract — saves still use the underlying HashMap.
    #[must_use]
    pub fn list_components(&self) -> Vec<&ComponentDefinition> {
        let mut out: Vec<&ComponentDefinition> = self.components.values().collect();
        out.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        out
    }

    /// Add a variant to an existing component.
    pub fn add_component_variant(
        &mut self,
        component_id: Uuid,
        variant: crate::component::ComponentVariant,
    ) -> Result<Uuid, ProjectError> {
        let comp = self
            .components
            .get_mut(&component_id)
            .ok_or(ProjectError::ComponentNotFound(component_id))?;
        let vid = comp.add_variant(variant);
        self.touch_modified();
        Ok(vid)
    }

    /// Remove a component. Existing `NodeType::ComponentLayer` nodes
    /// referencing the deleted component become orphaned (they keep
    /// their stored metadata but lookups via `get_component` will
    /// fail); detach instances before deletion if that matters.
    pub fn remove_component(&mut self, id: Uuid) -> Result<(), ProjectError> {
        if self.components.remove(&id).is_none() {
            return Err(ProjectError::ComponentNotFound(id));
        }
        self.touch_modified();
        Ok(())
    }

    fn touch_modified(&mut self) {
        self.modified_at = Utc::now();
    }
}

// ----------------------------------------------------------------------
// Layout Studio templates (Phase 2)
// ----------------------------------------------------------------------

/// High-level grouping of [`LayoutTemplate`]s for the template-picker UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateCategory {
    PitchDeck,
    Proposal,
    Brochure,
    Flyer,
    Report,
    Custom,
}

/// What a [`TemplateSectionDef`] generates on a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionKind {
    Title,
    Subtitle,
    BodyText,
    Image,
    Chart,
    Footer,
    PageNumber,
}

/// A region inside a template page — converted to a real node when
/// the template is applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct TemplateSectionDef {
    pub kind: SectionKind,
    pub bounds: crate::node::Bounds,
    pub placeholder_text: Option<String>,
}

/// A page inside a [`LayoutTemplate`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct TemplatePageDef {
    pub name: String,
    pub page_size: crate::node::PageSize,
    pub orientation: crate::node::PageOrientation,
    pub sections: Vec<TemplateSectionDef>,
}

/// A reusable Layout Studio template: deck, proposal, brochure, etc.
///
/// Templates ship as built-ins in [`builtin_layout_templates`]; future
/// versions can persist user-authored templates to disk and merge them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct LayoutTemplate {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub category: TemplateCategory,
    pub pages: Vec<TemplatePageDef>,
    pub design_tokens: Option<DesignTokens>,
}

/// Built-in templates: Pitch Deck (16:9), Proposal (A4), Brochure (A4 tri-fold landscape).
///
/// Ids are stable across calls so the UI can keep selection state
/// when re-listing.
/// Section definitions for a single Pitch-Deck slide — title hero, body
/// body, page number. Bounds are in document pixels (96 dpi); a 16:9
/// slide is 960×540 px.
fn pitch_template_sections(title: &str, body: &str) -> Vec<TemplateSectionDef> {
    vec![
        TemplateSectionDef {
            kind: SectionKind::Title,
            bounds: crate::node::Bounds::new(60.0, 60.0, 840.0, 80.0),
            placeholder_text: Some(title.to_string()),
        },
        TemplateSectionDef {
            kind: SectionKind::BodyText,
            bounds: crate::node::Bounds::new(60.0, 180.0, 840.0, 280.0),
            placeholder_text: Some(body.to_string()),
        },
        TemplateSectionDef {
            kind: SectionKind::PageNumber,
            bounds: crate::node::Bounds::new(880.0, 500.0, 60.0, 20.0),
            placeholder_text: None,
        },
    ]
}

/// Section definitions for a Proposal page — title, body, footer, page
/// number. A4 portrait is ≈ 794×1123 px @ 96 dpi.
fn proposal_template_sections(name: &str) -> Vec<TemplateSectionDef> {
    vec![
        TemplateSectionDef {
            kind: SectionKind::Title,
            bounds: crate::node::Bounds::new(60.0, 80.0, 670.0, 60.0),
            placeholder_text: Some(name.to_string()),
        },
        TemplateSectionDef {
            kind: SectionKind::BodyText,
            bounds: crate::node::Bounds::new(60.0, 170.0, 670.0, 880.0),
            placeholder_text: Some("Body content".to_string()),
        },
        TemplateSectionDef {
            kind: SectionKind::Footer,
            bounds: crate::node::Bounds::new(60.0, 1080.0, 550.0, 40.0),
            placeholder_text: Some("Confidential".to_string()),
        },
        TemplateSectionDef {
            kind: SectionKind::PageNumber,
            bounds: crate::node::Bounds::new(680.0, 1080.0, 50.0, 40.0),
            placeholder_text: None,
        },
    ]
}

/// Section definitions for a Brochure panel — title, image, body. A4
/// landscape is ≈ 1123×794 px @ 96 dpi.
fn brochure_template_sections(name: &str) -> Vec<TemplateSectionDef> {
    vec![
        TemplateSectionDef {
            kind: SectionKind::Title,
            bounds: crate::node::Bounds::new(50.0, 60.0, 1020.0, 80.0),
            placeholder_text: Some(name.to_string()),
        },
        TemplateSectionDef {
            kind: SectionKind::Image,
            bounds: crate::node::Bounds::new(50.0, 180.0, 500.0, 380.0),
            placeholder_text: None,
        },
        TemplateSectionDef {
            kind: SectionKind::BodyText,
            bounds: crate::node::Bounds::new(580.0, 180.0, 490.0, 380.0),
            placeholder_text: Some("Body text".to_string()),
        },
    ]
}

#[must_use]
pub fn builtin_layout_templates() -> Vec<LayoutTemplate> {
    let pitch_pages = [
        ("Cover", "Cover title page"),
        ("Problem", "Describe the problem"),
        ("Solution", "Your solution"),
        ("Market", "Market size & opportunity"),
        ("Product", "Product overview"),
        ("Business Model", "How you make money"),
        ("Team", "Who we are"),
        ("Ask", "Investment ask"),
        ("Contact", "Contact details"),
    ];
    let proposal_pages = [
        "Cover",
        "Executive Summary",
        "Scope",
        "Timeline",
        "Pricing",
        "Terms",
        "Contact",
    ];
    let brochure_pages = [
        "Front Panel",
        "Inside Left",
        "Inside Center",
        "Inside Right",
        "Back Panel",
        "Mailer Panel",
    ];

    vec![
        LayoutTemplate {
            id: Uuid::parse_str("11111111-1111-4111-8111-111111111111")
                .expect("static template uuid is valid"),
            name: "Pitch Deck".to_string(),
            description: "9-slide investor pitch deck in 16:9.".to_string(),
            category: TemplateCategory::PitchDeck,
            pages: pitch_pages
                .iter()
                .map(|(name, body)| TemplatePageDef {
                    name: (*name).to_string(),
                    page_size: crate::node::PageSize::Presentation16x9,
                    orientation: crate::node::PageOrientation::Landscape,
                    sections: pitch_template_sections(name, body),
                })
                .collect(),
            design_tokens: None,
        },
        LayoutTemplate {
            id: Uuid::parse_str("22222222-2222-4222-8222-222222222222")
                .expect("static template uuid is valid"),
            name: "Proposal".to_string(),
            description: "Multi-page proposal in A4 portrait.".to_string(),
            category: TemplateCategory::Proposal,
            pages: proposal_pages
                .iter()
                .map(|name| TemplatePageDef {
                    name: (*name).to_string(),
                    page_size: crate::node::PageSize::A4,
                    orientation: crate::node::PageOrientation::Portrait,
                    sections: proposal_template_sections(name),
                })
                .collect(),
            design_tokens: None,
        },
        LayoutTemplate {
            id: Uuid::parse_str("33333333-3333-4333-8333-333333333333")
                .expect("static template uuid is valid"),
            name: "Brochure".to_string(),
            description: "Tri-fold brochure in A4 landscape.".to_string(),
            category: TemplateCategory::Brochure,
            pages: brochure_pages
                .iter()
                .map(|name| TemplatePageDef {
                    name: (*name).to_string(),
                    page_size: crate::node::PageSize::A4,
                    orientation: crate::node::PageOrientation::Landscape,
                    sections: brochure_template_sections(name),
                })
                .collect(),
            design_tokens: None,
        },
    ]
}

impl Project {
    /// Apply a [`LayoutTemplate`] by creating one page per
    /// [`TemplatePageDef`] (with the template's layout) and one
    /// artboard child holding placeholder layers for each section.
    ///
    /// Returns the new page ids in template order.
    pub fn apply_layout_template(
        &mut self,
        template: &LayoutTemplate,
    ) -> Result<Vec<Uuid>, ProjectError> {
        let px_per_mm = 96.0 / 25.4;
        let mut created = Vec::with_capacity(template.pages.len());
        for (idx, def) in template.pages.iter().enumerate() {
            let layout = crate::node::PageLayout {
                page_size: def.page_size.clone(),
                orientation: def.orientation,
                margins: crate::node::Margins::default(),
                master_page_id: None,
                page_number: Some(u32::try_from(idx + 1).unwrap_or(u32::MAX)),
            };
            let (w_mm, h_mm) = layout.dimensions_mm();
            let mut page = Node::new(NodeType::Page, def.name.clone());
            page.bounds = Bounds::new(0.0, 0.0, w_mm * px_per_mm, h_mm * px_per_mm);
            page.set_page_layout(&layout);
            let page_id = page.id;
            self.document.insert_node(page)?;

            // One artboard per page, sized to the page.
            let mut artboard = Node::new(NodeType::Artboard, format!("{} / Content", def.name));
            artboard.parent_id = Some(page_id);
            artboard.bounds = Bounds::new(0.0, 0.0, w_mm * px_per_mm, h_mm * px_per_mm);
            let artboard_id = self.document.insert_node(artboard)?;

            // Materialise each section as a placeholder node under the artboard.
            for section in &def.sections {
                let node_type = match section.kind {
                    SectionKind::Title
                    | SectionKind::Subtitle
                    | SectionKind::BodyText
                    | SectionKind::Footer
                    | SectionKind::PageNumber => NodeType::TextLayer,
                    SectionKind::Image | SectionKind::Chart => NodeType::RasterLayer,
                };
                let label = match section.kind {
                    SectionKind::Title => "Title",
                    SectionKind::Subtitle => "Subtitle",
                    SectionKind::BodyText => "Body",
                    SectionKind::Image => "Image",
                    SectionKind::Chart => "Chart",
                    SectionKind::Footer => "Footer",
                    SectionKind::PageNumber => "Page #",
                };
                let mut node = Node::new(node_type, label);
                node.parent_id = Some(artboard_id);
                node.bounds = section.bounds;
                if let Some(text) = &section.placeholder_text {
                    node.metadata.insert(
                        "placeholder_text".to_string(),
                        serde_json::Value::String(text.clone()),
                    );
                }
                node.metadata.insert(
                    "template_section_kind".to_string(),
                    serde_json::to_value(section.kind).unwrap_or(serde_json::Value::Null),
                );
                self.document.insert_node(node)?;
            }

            created.push(page_id);
        }

        if let Some(tokens) = &template.design_tokens {
            self.design_tokens = tokens.clone();
        }
        self.touch_modified();
        Ok(created)
    }
}

/// Built-in export presets a brand-new project starts with. Exposed
/// via [`Project::install_default_export_presets`]; never called
/// implicitly because every call generates fresh UUIDs.
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
        assert!(
            p.export_presets.is_empty(),
            "new() must not auto-generate presets with fresh UUIDs (footgun on reopen)"
        );
        assert!(p.brand_kits.is_empty());
    }

    /// Regression test for `ANALYSIS_0003` on PR #2.
    ///
    /// `Project::new` previously hard-coded `OperationLog::default()`
    /// (256-deep), silently ignoring `RuntimeConfig::max_undo_depth`
    /// even though `DeviceTier::default_undo_depth` returns 32 on
    /// Tier 0 and 1024 on Tier 3. The `with_max_undo_depth`
    /// constructor is the production entry point that threads the
    /// device-tier budget into the log; this test pins the boundary
    /// behavior so a future "just call `new` everywhere" refactor
    /// can't silently regress the resource-awareness contract from
    /// `ARCHITECTURE.md` §14.
    #[test]
    fn with_max_undo_depth_threads_through_to_log() {
        let tier0 = Project::with_max_undo_depth("tier0", 32);
        assert_eq!(tier0.operation_log.max_depth(), 32);
        let tier3 = Project::with_max_undo_depth("tier3", 1024);
        assert_eq!(tier3.operation_log.max_depth(), 1024);
        // `new` still uses the documented 256 default so existing
        // callers (tests, examples) don't shift behaviour.
        let default_p = Project::new("default");
        assert_eq!(default_p.operation_log.max_depth(), 256);
        // `0` is clamped to `1` (a useless but well-defined edge),
        // matching `OperationLog::new`.
        let edge = Project::with_max_undo_depth("edge", 0);
        assert_eq!(edge.operation_log.max_depth(), 1);
    }

    #[test]
    fn install_default_export_presets_populates_after_new() {
        let mut p = Project::new("My Project");
        assert!(p.export_presets.is_empty());
        p.install_default_export_presets();
        assert!(!p.export_presets.is_empty());
    }

    #[test]
    fn install_default_export_presets_regenerates_ids() {
        let mut a = Project::new("a");
        a.install_default_export_presets();
        let mut b = Project::new("b");
        b.install_default_export_presets();
        // Different Project instances must get different preset ids,
        // but the count and shape match.
        assert_eq!(a.export_presets.len(), b.export_presets.len());
        assert_ne!(a.export_presets[0].id, b.export_presets[0].id);
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
    fn create_master_page_and_apply_to_content_page() {
        let mut p = Project::new("doc");
        let layout = crate::node::PageLayout::new(
            crate::node::PageSize::A4,
            crate::node::PageOrientation::Portrait,
        );
        let master = p.create_master_page("Master A", layout).expect("master");
        let masters = p.list_master_pages();
        assert_eq!(masters.len(), 1);
        assert_eq!(masters[0].id, master);

        // Content page lacks layout until we attach.
        let content = p.add_page("Page 1").expect("content");
        assert!(p.resolve_master_page(content).is_none());
        p.apply_master_page(content, master).expect("apply");
        assert_eq!(p.resolve_master_page(content), Some(master));

        // Detaching clears the master.
        p.detach_master_page(content).expect("detach");
        assert!(p.resolve_master_page(content).is_none());
    }

    #[test]
    fn apply_master_page_rejects_non_master_target() {
        let mut p = Project::new("doc");
        let not_master = p.add_page("Page").expect("page");
        let content = p.add_page("Other").expect("content");
        let err = p.apply_master_page(content, not_master).unwrap_err();
        match err {
            ProjectError::Document(DocumentError::WrongNodeType { .. }) => {}
            other => panic!("unexpected: {other}"),
        }
    }

    #[test]
    fn builtin_layout_templates_produce_real_pages() {
        let templates = builtin_layout_templates();
        assert_eq!(templates.len(), 3);
        let pitch = templates
            .iter()
            .find(|t| t.category == TemplateCategory::PitchDeck)
            .expect("pitch deck template exists");
        assert_eq!(pitch.pages.len(), 9);

        let mut p = Project::new("deck");
        let pages = p.apply_layout_template(pitch).expect("apply");
        assert_eq!(pages.len(), 9);
        // Every created page has a layout in 16:9 landscape.
        for pid in pages {
            let page = p.document.get_node(pid).expect("page exists");
            let layout = page.page_layout().expect("page has layout");
            assert_eq!(layout.orientation, crate::node::PageOrientation::Landscape);
            assert_eq!(layout.page_size, crate::node::PageSize::Presentation16x9);
        }
    }

    #[test]
    fn layout_template_round_trip_through_json() {
        let templates = builtin_layout_templates();
        let json = serde_json::to_string(&templates).expect("serialize");
        let back: Vec<LayoutTemplate> = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, templates);
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
        let mut p = Project::new("p");
        p.install_default_export_presets();
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
        p.install_default_export_presets();
        p.add_page("Home").expect("page");
        let s = serde_json::to_string(&p).expect("serialize");
        let p2: Project = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(p.id, p2.id);
        assert_eq!(p.document.node_count(), p2.document.node_count());
        assert_eq!(p.export_presets.len(), p2.export_presets.len());
        // ids must round-trip 1:1 — they're not regenerated on deserialize.
        let original_ids: Vec<_> = p.export_presets.iter().map(|x| x.id).collect();
        let restored_ids: Vec<_> = p2.export_presets.iter().map(|x| x.id).collect();
        assert_eq!(original_ids, restored_ids);
    }

    #[test]
    fn brand_kit_not_found_errors() {
        let p = Project::new("p");
        let err = p.brand_kit(Uuid::new_v4()).expect_err("missing");
        assert!(matches!(err, ProjectError::BrandKitNotFound(_)));
    }

    #[test]
    fn register_and_list_components_sorted_by_name() {
        use crate::component::ComponentDefinition;
        let mut p = Project::new("p");
        let banana = ComponentDefinition::new("Banana");
        let apple = ComponentDefinition::new("Apple");
        let banana_id = banana.id;
        let apple_id = apple.id;
        p.register_component(banana);
        p.register_component(apple);
        let listed = p.list_components();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, apple_id);
        assert_eq!(listed[1].id, banana_id);
    }

    #[test]
    fn add_component_variant_persists() {
        use crate::component::{ComponentDefinition, ComponentVariant};
        let mut p = Project::new("p");
        let def = ComponentDefinition::new("Button");
        let cid = p.register_component(def);
        let vid = p
            .add_component_variant(cid, ComponentVariant::new("Hover"))
            .expect("variant");
        let c = p.get_component(cid).expect("component");
        assert_eq!(c.variants.len(), 2);
        assert!(c.variant(vid).is_some());
    }

    #[test]
    fn add_variant_to_missing_component_errors() {
        use crate::component::ComponentVariant;
        let mut p = Project::new("p");
        let err = p
            .add_component_variant(Uuid::new_v4(), ComponentVariant::new("Hover"))
            .expect_err("missing");
        assert!(matches!(err, ProjectError::ComponentNotFound(_)));
    }

    #[test]
    fn remove_component_drops_definition() {
        use crate::component::ComponentDefinition;
        let mut p = Project::new("p");
        let def = ComponentDefinition::new("Button");
        let cid = p.register_component(def);
        p.remove_component(cid).expect("remove");
        assert!(p.get_component(cid).is_none());
        let again = p.remove_component(cid).expect_err("already gone");
        assert!(matches!(again, ProjectError::ComponentNotFound(_)));
    }

    #[test]
    fn project_with_components_roundtrips_through_json() {
        use crate::component::ComponentDefinition;
        let mut p = Project::new("p");
        let cid = p.register_component(ComponentDefinition::new("Button"));
        let s = serde_json::to_string(&p).expect("serialize");
        let back: Project = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(back.components.len(), 1);
        assert!(back.get_component(cid).is_some());
    }
}

//! `kcreate_core` — the foundational data model for `KCreate`.
//!
//! This crate contains the canonical, renderer-independent types used
//! across the workspace:
//!
//! - [`node`] — the `Node` struct, transforms, bounds, styles, effects,
//!   constraints, and blend modes.
//! - [`document`] — a [`DocumentGraph`] with O(1) node lookups and an
//!   explicit parent/child tree (no recursive ownership; learned from
//!   `ux-open-pencil`).
//! - [`operation`] — append-only [`OperationLog`] with undo/redo and a
//!   bounded history depth.
//! - [`project`] — [`Project`], the top-level container that ties the
//!   document graph, operation log, brand kits, design tokens, and
//!   export presets together.
//! - [`config`] — runtime configuration: platform detection, device
//!   tier, low-resource mode.
//!
//! The crate intentionally has **no dependency** on `kcreate_renderer`
//! or anything napi-related, so it can be reused by headless tools,
//! exporters, and integration tests without paying the cost of pulling
//! in wgpu.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod align;
pub mod annotation;
pub mod assets;
pub mod color;
pub mod component;
pub mod config;
pub mod document;
pub mod icc;
pub mod marketplace;
pub mod node;
pub mod operation;
pub mod operation_compress;
pub mod project;
pub mod template_library;
pub mod token_binding;

pub use align::{
    align_bounds, distribute_bounds, distribute_gap, union_bounds, Align, AlignDelta,
    DistributeAxis,
};
pub use annotation::{Annotation, AnnotationFilter, AnnotationPosition};
pub use assets::{AssetCategory, AssetDef, CategoryInfo};
pub use color::{
    cmyk_to_srgb, color_distance_cie76, hsl_to_srgb, lab_to_srgb, linear_to_srgb, srgb_to_cmyk,
    srgb_to_hsl, srgb_to_lab, srgb_to_linear, srgb_to_xyz_d65, xyz_d65_to_srgb, CatalogParseStats,
    Color, ColorSettings, IccProfile, RenderingIntent,
};
pub use component::{
    ComponentDefinition, ComponentError, ComponentInstance, ComponentVariant,
    COMPONENT_INSTANCE_METADATA_KEY,
};
pub use config::{DeviceTier, Platform, RuntimeConfig};
pub use document::{DocumentError, DocumentGraph};
pub use icc::{cmyk_to_srgb_profiled, convert_color, srgb_to_cmyk_profiled, ColorTransform};
pub use marketplace::{LocalMarketplace, MarketplaceError, TemplateManifest, TemplateSource};
pub use node::{
    standard_presets, ArtboardPreset, BlendMode, Bounds, Constraint, Constraints, Effect,
    FillStyle, FrameInsets, GradientKind, GradientStop, Interaction, InteractionAction,
    InteractionTrigger, LineCap, LineJoin, Margins, Node, NodeStyle, NodeType, OpenTypeFeatures,
    PageLayout, PageOrientation, PageSize, Point2D, PresetCategory, RgbaColor, StrokeStyle,
    TextAutoSize, TextFrameOptions, TextOverflow, TextWrapMode, Transform2D, VerticalAlign,
    INTERACTIONS_METADATA_KEY, MASTER_PAGE_METADATA_KEY, OPENTYPE_FEATURES_METADATA_KEY,
    PAGE_LAYOUT_METADATA_KEY, TEXT_FRAME_METADATA_KEY,
};
pub use operation::{Operation, OperationLog};
pub use operation_compress::{
    apply_diff, compress_operation, compute_diff, expand_operation, materialize_blob_refs,
    replace_blobs_with_refs, CompressedOperation, DiffOp, PathSegment, BLOB_REF_MARKER,
    INLINE_BLOB_MARKER,
};
pub use project::{
    builtin_layout_templates, BrandKit, DesignTokens, ExportFormat, ExportPreset, FontRef,
    LayoutTemplate, NamedColor, Project, ProjectError, SectionKind, ShadowToken, TemplateCategory,
    TemplatePageDef, TemplateSectionDef, TypographyToken,
};
pub use template_library::{
    bundled_templates, seed_bundled_templates, BundledTemplate, TemplateContent, TemplateItem,
};
pub use token_binding::{
    bind_token, nodes_bound_to, propagate_single_token, propagate_token_changes, refresh_style,
    unbind_token, BindError, PropagationReport, StyleProperty, TokenKind,
};

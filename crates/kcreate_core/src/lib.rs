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

pub mod component;
pub mod config;
pub mod document;
pub mod node;
pub mod operation;
pub mod project;

pub use component::{
    ComponentDefinition, ComponentError, ComponentInstance, ComponentVariant,
    COMPONENT_INSTANCE_METADATA_KEY,
};
pub use config::{DeviceTier, Platform, RuntimeConfig};
pub use document::{DocumentError, DocumentGraph};
pub use node::{
    standard_presets, ArtboardPreset, BlendMode, Bounds, Constraint, Constraints, Effect,
    FillStyle, GradientKind, GradientStop, Node, NodeStyle, NodeType, Point2D, PresetCategory,
    RgbaColor, StrokeStyle, Transform2D,
};
pub use operation::{Operation, OperationLog};
pub use project::{
    BrandKit, DesignTokens, ExportFormat, ExportPreset, FontRef, NamedColor, Project, ProjectError,
    ShadowToken, TypographyToken,
};

//! Reusable components — a definition + N variants, instantiated by
//! `NodeType::ComponentLayer` nodes.
//!
//! A component is a named, reusable design fragment that can be
//! dropped into any document. Each component has:
//!
//! * a stable [`Uuid`] used to wire instances back to the definition
//! * a human name and description
//! * one or more **variants** (e.g. *Default*, *Hover*, *Active*,
//!   *Disabled*) — each variant is a property bag
//! * a *default* variant pointing at the variant id rendered when an
//!   instance doesn't override it
//!
//! When a [`Node`] of type [`crate::node::NodeType::ComponentLayer`]
//! references a component, the bridge stores a serialized
//! [`ComponentInstance`] under the node's metadata key
//! [`COMPONENT_INSTANCE_METADATA_KEY`]. The instance carries the
//! component id, the currently-displayed variant id, and any
//! per-instance overrides.
//!
//! The variant `properties` map is intentionally typed as
//! [`serde_json::Value`] so the renderer/UI can layer arbitrary
//! design tokens (colors, sizes, copy, …) without baking the schema
//! into Rust. Validation happens at the consumer.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

/// Metadata key the bridge writes a [`ComponentInstance`] payload to
/// on a [`crate::node::NodeType::ComponentLayer`] node.
pub const COMPONENT_INSTANCE_METADATA_KEY: &str = "component_instance";

/// One variant of a component (e.g. "Default", "Hover").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)] // free-form JSON properties
pub struct ComponentVariant {
    pub id: Uuid,
    pub name: String,
    /// Per-variant property bag. Free-form JSON: design tokens, copy
    /// strings, raster ids — whatever the renderer / UI consume.
    #[serde(default)]
    pub properties: HashMap<String, JsonValue>,
}

impl ComponentVariant {
    /// Create a new variant with a fresh id and an empty property bag.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            properties: HashMap::new(),
        }
    }
}

/// A reusable component definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)] // transitively via ComponentVariant
pub struct ComponentDefinition {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// At least one entry. `default_variant_id` must point at one of
    /// these. Order is preserved for UI purposes.
    pub variants: Vec<ComponentVariant>,
    pub default_variant_id: Uuid,
    /// Source nodes captured when the component was created (e.g.
    /// from a selection). The bridge replays these subtrees when
    /// instantiating the component. Empty for components built from
    /// scratch in the UI.
    #[serde(default)]
    pub source_node_ids: Vec<Uuid>,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
}

impl ComponentDefinition {
    /// Build a new component with a single auto-generated "Default"
    /// variant.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        let default_variant = ComponentVariant::new("Default");
        let default_variant_id = default_variant.id;
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            description: String::new(),
            variants: vec![default_variant],
            default_variant_id,
            source_node_ids: Vec::new(),
            created_at: now,
            modified_at: now,
        }
    }

    /// Look up a variant by id.
    #[must_use]
    pub fn variant(&self, id: Uuid) -> Option<&ComponentVariant> {
        self.variants.iter().find(|v| v.id == id)
    }

    /// Mutably look up a variant by id.
    pub fn variant_mut(&mut self, id: Uuid) -> Option<&mut ComponentVariant> {
        self.variants.iter_mut().find(|v| v.id == id)
    }

    /// Append a variant and return its id.
    pub fn add_variant(&mut self, variant: ComponentVariant) -> Uuid {
        let id = variant.id;
        self.variants.push(variant);
        self.touch();
        id
    }

    /// Remove a variant by id. Removing the default variant is
    /// rejected (the caller should switch `default_variant_id` to a
    /// surviving variant first).
    pub fn remove_variant(&mut self, id: Uuid) -> Result<(), ComponentError> {
        if id == self.default_variant_id {
            return Err(ComponentError::CannotRemoveDefaultVariant);
        }
        let initial = self.variants.len();
        self.variants.retain(|v| v.id != id);
        if self.variants.len() == initial {
            return Err(ComponentError::VariantNotFound(id));
        }
        self.touch();
        Ok(())
    }

    /// Update the default variant. The new id must reference an
    /// existing variant.
    pub fn set_default_variant(&mut self, id: Uuid) -> Result<(), ComponentError> {
        if self.variant(id).is_none() {
            return Err(ComponentError::VariantNotFound(id));
        }
        self.default_variant_id = id;
        self.touch();
        Ok(())
    }

    fn touch(&mut self) {
        self.modified_at = Utc::now();
    }
}

/// A node-level reference to a component. Stored on the component
/// node's metadata bag (see [`COMPONENT_INSTANCE_METADATA_KEY`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)] // free-form JSON overrides
pub struct ComponentInstance {
    pub definition_id: Uuid,
    pub active_variant_id: Uuid,
    /// Per-instance overrides. Looked up before the variant's own
    /// `properties`. Free-form JSON for the same reason as
    /// [`ComponentVariant::properties`].
    #[serde(default)]
    pub overrides: HashMap<String, JsonValue>,
}

impl ComponentInstance {
    /// Construct an instance that displays the component's default
    /// variant with no overrides.
    #[must_use]
    pub fn new(definition: &ComponentDefinition) -> Self {
        Self {
            definition_id: definition.id,
            active_variant_id: definition.default_variant_id,
            overrides: HashMap::new(),
        }
    }
}

/// Errors from the component subsystem.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ComponentError {
    #[error("component {0} not found")]
    NotFound(Uuid),
    #[error("variant {0} not found")]
    VariantNotFound(Uuid),
    #[error("cannot remove the default variant — switch the default first")]
    CannotRemoveDefaultVariant,
    #[error("component instance metadata on node {0} is malformed: {1}")]
    InvalidInstance(Uuid, String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_component_has_default_variant() {
        let c = ComponentDefinition::new("Button");
        assert_eq!(c.name, "Button");
        assert_eq!(c.variants.len(), 1);
        assert_eq!(c.variants[0].name, "Default");
        assert_eq!(c.default_variant_id, c.variants[0].id);
    }

    #[test]
    fn add_and_lookup_variant() {
        let mut c = ComponentDefinition::new("Button");
        let v_id = c.add_variant(ComponentVariant::new("Hover"));
        assert_eq!(c.variants.len(), 2);
        assert!(c.variant(v_id).is_some());
        assert_eq!(c.variant(v_id).expect("found").name, "Hover");
    }

    #[test]
    fn cannot_remove_default_variant() {
        let mut c = ComponentDefinition::new("Button");
        let def = c.default_variant_id;
        let err = c.remove_variant(def).expect_err("must reject");
        assert_eq!(err, ComponentError::CannotRemoveDefaultVariant);
    }

    #[test]
    fn remove_then_switch_default() {
        let mut c = ComponentDefinition::new("Button");
        let hover = c.add_variant(ComponentVariant::new("Hover"));
        c.set_default_variant(hover).expect("switch default");
        // Now the original "Default" variant is no longer the
        // default and can be safely removed.
        let original = c
            .variants
            .iter()
            .find(|v| v.name == "Default")
            .expect("default variant")
            .id;
        c.remove_variant(original).expect("remove");
        assert_eq!(c.variants.len(), 1);
        assert_eq!(c.default_variant_id, hover);
    }

    #[test]
    fn instance_starts_on_default_variant() {
        let c = ComponentDefinition::new("Button");
        let inst = ComponentInstance::new(&c);
        assert_eq!(inst.definition_id, c.id);
        assert_eq!(inst.active_variant_id, c.default_variant_id);
        assert!(inst.overrides.is_empty());
    }

    #[test]
    fn instance_roundtrips_through_json() {
        let c = ComponentDefinition::new("Card");
        let mut inst = ComponentInstance::new(&c);
        inst.overrides.insert(
            "title".to_string(),
            serde_json::Value::String("Hello".to_string()),
        );
        let s = serde_json::to_string(&inst).expect("serialize");
        let back: ComponentInstance = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(back, inst);
    }
}

//! KCreate plugin sandbox.
//!
//! Plugins are declared via a `manifest.json` file inside a plugin
//! directory; the entry-point is a `.wasm` module. The runtime here is
//! pure-Rust (`wasmi`) and has no access to the filesystem, network,
//! or any KCreate state — plugins communicate via two buffers
//! (`kcreate_get_input` / `kcreate_set_output`) plus a logging hook.
//!
//! The crate has no editing-path dependencies; it can be loaded by the
//! bridge or run standalone for testing.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod bundled;
pub mod context;
pub mod js_panel;
pub mod manifest;
pub mod marketplace;
pub mod registry;
pub mod trust;
pub mod wasm_runtime;

pub use bundled::{
    bundled_plugin, bundled_plugins, bundled_trust_store, bundled_trusted_keys, BundledPlugin,
};
pub use context::{
    resolve_document_query, AssetLoader, DocumentQuery, PluginContext, PluginProposal,
    ProposedMutation,
};
pub use js_panel::{
    JsPanelConfig, JsPanelInfo, JsPanelMessage, JsPanelMessageOutcome, PanelPosition,
};
pub use manifest::{PluginManifest, PluginPermission, PluginSignature, PluginType};
pub use registry::{PluginRegistry, RegistryError, SignatureStatus};
pub use trust::{encode_b64 as trust_encode_b64, TrustError, TrustStore, TrustedKey};
pub use wasm_runtime::{PluginOutput, WasmPluginError, WasmPluginRuntime};

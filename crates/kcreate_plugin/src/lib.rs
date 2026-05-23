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

pub mod context;
pub mod manifest;
pub mod registry;
pub mod wasm_runtime;

pub use context::{
    resolve_document_query, AssetLoader, DocumentQuery, PluginContext, PluginProposal,
    ProposedMutation,
};
pub use manifest::{PluginManifest, PluginPermission, PluginType};
pub use registry::{PluginRegistry, RegistryError};
pub use wasm_runtime::{PluginOutput, WasmPluginError, WasmPluginRuntime};

//! JS panel plugin configuration and lifecycle model.
//!
//! A JS panel plugin runs inside an Electron `BrowserView` (or
//! sandboxed `<webview>`) that is allocated and torn down by the
//! main process. This crate intentionally does **not** execute any
//! JavaScript itself — it owns the *type* of the panel config and
//! the *types* of messages travelling between the panel and the
//! bridge.  The Electron host (in `apps/desktop/main/src/main.ts`)
//! is what actually creates the view, loads the HTML, and wires the
//! plugin preload script.
//!
//! The wire format is shared with the renderer through
//! `apps/desktop/shared/scene.ts`, so any field change here must
//! propagate there as well (AGENTS.md rule 4).

use serde::{Deserialize, Serialize};

use crate::manifest::PluginPermission;

/// Where in the editor chrome a JS panel docks. The Electron host
/// translates these positions into concrete `BrowserView` bounds —
/// this crate just declares the contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelPosition {
    /// Dock inside the right sidebar, below the standard property
    /// panels.
    RightSidebar,
    /// Dock as a tab inside the bottom panel (alongside the timeline
    /// / console / etc.).
    BottomPanel,
    /// Float as an independent always-on-top window.
    FloatingWindow,
}

/// The Phase 2 schema for a JS panel plugin's `manifest.json`
/// `js_panel` section. The plugin's manifest still uses the same
/// top-level `PluginManifest` shape (with `type = "js_panel"`); this
/// struct describes the additional metadata stored alongside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsPanelConfig {
    /// Path to the panel's HTML entry point, relative to the plugin
    /// directory. The plugin author bundles all required JS / CSS
    /// alongside this file; the Electron host loads it via `file://`.
    pub entry_html: String,
    /// Title shown in the panel header.
    pub panel_title: String,
    /// Where the panel docks.
    pub panel_position: PanelPosition,
    /// Desired initial width, in CSS pixels. The host clamps this to
    /// the available chrome space.
    pub width: u32,
    /// Desired initial height, in CSS pixels.
    pub height: u32,
    /// Permissions the panel requires. The host enforces these on
    /// every relayed message; messages whose required permission is
    /// not granted are dropped with a denial log line in the same
    /// shape WASM plugins emit.
    #[serde(default)]
    pub permissions: Vec<PluginPermission>,
}

impl JsPanelConfig {
    /// True iff this panel has been granted `permission`. The
    /// Electron host calls this on every inbound `postMessage` before
    /// invoking the bridge.
    #[must_use]
    pub fn has(&self, permission: PluginPermission) -> bool {
        self.permissions.contains(&permission)
    }
}

/// Description of an active JS panel plugin, returned by
/// `plugin_js_list()` so the renderer can populate the plugin
/// manager and decide what to mount.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsPanelInfo {
    /// Plugin ID, matches `PluginManifest::id`.
    pub id: String,
    /// Display name from `PluginManifest::name`.
    pub name: String,
    /// Plugin version from `PluginManifest::version`.
    pub version: String,
    /// The panel config from the plugin's manifest.
    pub config: JsPanelConfig,
    /// Whether the plugin is currently enabled in the registry. The
    /// renderer should only mount enabled panels.
    pub enabled: bool,
}

/// A message ferried between a JS panel and the bridge. JS panels
/// don't speak the WASM ABI — they exchange these tagged-JSON
/// messages over `postMessage`. The Electron preload (`plugin-preload.ts`)
/// wraps `window.kcreatePlugin.sendMessage(...)` and the bridge
/// validates the tag against the panel's declared permissions before
/// dispatching it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JsPanelMessage {
    /// `kcreate_read_document` equivalent — payload is a
    /// `context::DocumentQuery` JSON object. Requires
    /// `PluginPermission::ReadDocument`.
    ReadDocument { query: serde_json::Value },
    /// `kcreate_write_proposal` equivalent — payload is a single
    /// `context::ProposedMutation`. Requires
    /// `PluginPermission::WriteDocument`.
    WriteProposal { proposal: serde_json::Value },
    /// `kcreate_log` equivalent — purely informational, no
    /// permission gate.
    Log { message: String },
}

/// Outcome of a `JsPanelMessage` after the bridge has validated and
/// (where applicable) dispatched it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum JsPanelMessageOutcome {
    /// The message was accepted and any side effects have been
    /// recorded. `result` carries the response payload (e.g. for
    /// `ReadDocument`, the JSON the query resolved to).
    Ok { result: serde_json::Value },
    /// The panel lacked the required permission. `permission`
    /// identifies which one was missing so the panel can adjust its
    /// UI.
    Denied { permission: PluginPermission },
    /// The message was structurally invalid (bad JSON, unknown
    /// shape, etc.). `reason` is a short human-readable string.
    Invalid { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> JsPanelConfig {
        JsPanelConfig {
            entry_html: "panel.html".to_string(),
            panel_title: "Example".to_string(),
            panel_position: PanelPosition::RightSidebar,
            width: 320,
            height: 480,
            permissions: vec![PluginPermission::ReadDocument],
        }
    }

    #[test]
    fn config_round_trips_through_json() {
        let cfg = sample_config();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: JsPanelConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn config_permissions_default_to_empty() {
        let json = r#"{
            "entry_html": "panel.html",
            "panel_title": "X",
            "panel_position": "right_sidebar",
            "width": 1,
            "height": 1
        }"#;
        let cfg: JsPanelConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.permissions.is_empty());
        assert!(!cfg.has(PluginPermission::ReadDocument));
    }

    #[test]
    fn config_has_permission() {
        let cfg = sample_config();
        assert!(cfg.has(PluginPermission::ReadDocument));
        assert!(!cfg.has(PluginPermission::WriteDocument));
    }

    #[test]
    fn panel_position_serialises_snake_case() {
        let s = serde_json::to_string(&PanelPosition::FloatingWindow).unwrap();
        assert_eq!(s, "\"floating_window\"");
    }

    #[test]
    fn panel_info_round_trips() {
        let info = JsPanelInfo {
            id: "demo".to_string(),
            name: "Demo".to_string(),
            version: "0.1.0".to_string(),
            config: sample_config(),
            enabled: true,
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: JsPanelInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, back);
    }

    #[test]
    fn message_read_document_round_trips() {
        let msg = JsPanelMessage::ReadDocument {
            query: serde_json::json!({"type": "list_nodes"}),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: JsPanelMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn message_write_proposal_round_trips() {
        let msg = JsPanelMessage::WriteProposal {
            proposal: serde_json::json!({
                "type": "delete_node",
                "node_id": "00000000-0000-0000-0000-000000000000"
            }),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: JsPanelMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn message_log_round_trips() {
        let msg = JsPanelMessage::Log {
            message: "hello".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: JsPanelMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn outcome_denied_round_trips() {
        let outcome = JsPanelMessageOutcome::Denied {
            permission: PluginPermission::WriteDocument,
        };
        let json = serde_json::to_string(&outcome).unwrap();
        let back: JsPanelMessageOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(outcome, back);
    }

    #[test]
    fn outcome_ok_round_trips() {
        let outcome = JsPanelMessageOutcome::Ok {
            result: serde_json::json!([1, 2, 3]),
        };
        let json = serde_json::to_string(&outcome).unwrap();
        let back: JsPanelMessageOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(outcome, back);
    }

    #[test]
    fn outcome_invalid_round_trips() {
        let outcome = JsPanelMessageOutcome::Invalid {
            reason: "bad json".to_string(),
        };
        let json = serde_json::to_string(&outcome).unwrap();
        let back: JsPanelMessageOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(outcome, back);
    }
}

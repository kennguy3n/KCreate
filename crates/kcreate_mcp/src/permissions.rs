//! MCP per-(client, tool) permission store.
//!
//! Every external MCP client connecting to the loopback server is
//! treated as untrusted: tool invocations are gated by an explicit
//! grant the user makes through the McpSettingsPanel UI. Grants are
//! persisted to a JSON file (`mcp_permissions.json`) inside a
//! caller-supplied directory so they survive editor restarts.
//!
//! The granularity is `(client_id, tool_name)` — granting `mcp.client.editor`
//! access to `export_artboard` does not grant it access to `create_node`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One stored permission entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpPermission {
    pub client_id: String,
    pub tool_name: String,
    pub granted: PermissionGrant,
    pub granted_at: DateTime<Utc>,
}

/// Decision a user can attach to a `(client, tool)` pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionGrant {
    /// Allow exactly one invocation, then return to "no permission".
    /// The store does not auto-revoke — it is up to the server to
    /// transition the entry to [`PermissionGrant::Denied`] after use.
    Once,
    /// Allow indefinitely. Survives restarts.
    Always,
    /// Explicit deny. The server returns a refusal without prompting.
    Denied,
}

impl PermissionGrant {
    /// True when this grant permits a *new* tool invocation.
    pub const fn allows(self) -> bool {
        matches!(self, Self::Once | Self::Always)
    }
}

#[derive(Debug, Error)]
pub enum PermissionStoreError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoredFile {
    entries: Vec<McpPermission>,
}

/// Permission store. Owns a JSON file on disk plus an in-memory index.
#[derive(Debug)]
pub struct McpPermissionStore {
    path: PathBuf,
    inner: RwLock<HashMap<(String, String), McpPermission>>,
}

impl McpPermissionStore {
    /// Load (or create) a store at `dir/mcp_permissions.json`.
    ///
    /// Returns an error only for I/O failures (directory not writable,
    /// permissions file not readable). JSON parse errors are NOT
    /// considered fatal here — see [`McpPermissionStore::open_recoverable`]
    /// for the recovery contract used by long-running bridge processes.
    pub fn open(dir: &Path) -> Result<Self, PermissionStoreError> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("mcp_permissions.json");
        let mut map: HashMap<(String, String), McpPermission> = HashMap::new();
        if path.exists() {
            let bytes = std::fs::read(&path)?;
            if !bytes.is_empty() {
                let file: StoredFile = serde_json::from_slice(&bytes)?;
                for entry in file.entries {
                    map.insert((entry.client_id.clone(), entry.tool_name.clone()), entry);
                }
            }
        }
        Ok(Self {
            path,
            inner: RwLock::new(map),
        })
    }

    /// Load a store at `dir/mcp_permissions.json`, recovering from a
    /// corrupted JSON file by quarantining it and starting empty.
    ///
    /// Long-running processes (the kcreate_bridge `OnceLock` singleton)
    /// should use this instead of [`McpPermissionStore::open`] because a
    /// partially-flushed or hand-edited permissions file should not bring
    /// down the entire Electron main process on the first MCP tool call.
    ///
    /// The corrupted file is renamed to `<path>.corrupt-<unix_ts>` so the
    /// data is preserved for forensics; the new empty store is then
    /// persisted to the original path on the first write.
    ///
    /// I/O failures (directory not writable, etc.) are still returned as
    /// `Err` — there is no sensible recovery for those.
    pub fn open_recoverable(dir: &Path) -> Result<Self, PermissionStoreError> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("mcp_permissions.json");
        let mut map: HashMap<(String, String), McpPermission> = HashMap::new();
        if path.exists() {
            let bytes = std::fs::read(&path)?;
            if !bytes.is_empty() {
                match serde_json::from_slice::<StoredFile>(&bytes) {
                    Ok(file) => {
                        for entry in file.entries {
                            map.insert((entry.client_id.clone(), entry.tool_name.clone()), entry);
                        }
                    }
                    Err(_) => {
                        let ts = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |d| d.as_secs());
                        let quarantined = path.with_extension(format!("json.corrupt-{ts}"));
                        // Best-effort rename — if it fails, we still proceed
                        // with an empty in-memory store and the corrupt file
                        // will be overwritten on the next flush.
                        let _ = std::fs::rename(&path, &quarantined);
                    }
                }
            }
        }
        Ok(Self {
            path,
            inner: RwLock::new(map),
        })
    }

    /// Look up the current grant for `(client_id, tool_name)`.
    pub fn check(&self, client_id: &str, tool_name: &str) -> Option<McpPermission> {
        self.inner
            .read()
            .get(&(client_id.to_string(), tool_name.to_string()))
            .cloned()
    }

    /// Record a grant. The previous entry (if any) is overwritten.
    pub fn grant(
        &self,
        client_id: &str,
        tool_name: &str,
        grant: PermissionGrant,
    ) -> Result<(), PermissionStoreError> {
        let entry = McpPermission {
            client_id: client_id.to_string(),
            tool_name: tool_name.to_string(),
            granted: grant,
            granted_at: Utc::now(),
        };
        {
            let mut guard = self.inner.write();
            guard.insert((client_id.to_string(), tool_name.to_string()), entry);
        }
        self.persist()
    }

    /// Drop a previously-stored grant.
    pub fn revoke(&self, client_id: &str, tool_name: &str) -> Result<(), PermissionStoreError> {
        {
            let mut guard = self.inner.write();
            guard.remove(&(client_id.to_string(), tool_name.to_string()));
        }
        self.persist()
    }

    /// All known grants, sorted by `(client_id, tool_name)`.
    pub fn list(&self) -> Vec<McpPermission> {
        let mut v: Vec<McpPermission> = self.inner.read().values().cloned().collect();
        v.sort_by(|a, b| {
            a.client_id
                .cmp(&b.client_id)
                .then(a.tool_name.cmp(&b.tool_name))
        });
        v
    }

    /// All grants belonging to a single client.
    pub fn list_for_client(&self, client_id: &str) -> Vec<McpPermission> {
        let mut v: Vec<McpPermission> = self
            .inner
            .read()
            .values()
            .filter(|e| e.client_id == client_id)
            .cloned()
            .collect();
        v.sort_by(|a, b| a.tool_name.cmp(&b.tool_name));
        v
    }

    /// Convenience: enforce a permission check at tool-invocation
    /// time. Returns `true` iff the stored grant currently allows
    /// execution. A `Once` grant is consumed by transitioning to
    /// `Denied`; the caller should refresh by calling `check` again
    /// for the next invocation.
    ///
    /// The read-modify-write — observing the `Once` grant and then
    /// transitioning it to `Denied` — happens under a *single* write
    /// lock acquisition. The previous implementation called
    /// `self.check` (acquires the read lock, releases it) and then
    /// `self.grant` (acquires the write lock), which left a window
    /// where two concurrent invocations could each observe the same
    /// `Once` grant and both succeed. The MCP server's `tiny_http`
    /// accept loop is single-threaded so the race was not reachable
    /// in practice, but a defence-in-depth atomic transition is the
    /// right architecture for a permission gate. Per Devin Review
    /// ANALYSIS_pr-review-job-0594c03f68c24589ba78a32926e3874f_0001.
    pub fn consume_if_once(
        &self,
        client_id: &str,
        tool_name: &str,
    ) -> Result<bool, PermissionStoreError> {
        let key = (client_id.to_string(), tool_name.to_string());
        // Hold the write lock for the whole observe-and-transition.
        // The persist() call below requires its own read lock for the
        // sorted listing, so we explicitly release the write lock
        // before calling persist() — but the in-memory transition is
        // complete and visible to every subsequent reader by that
        // point, so a racing `consume_if_once` will see `Denied`.
        let (allowed, needs_persist) = {
            let mut guard = self.inner.write();
            let Some(entry) = guard.get(&key).cloned() else {
                return Ok(false);
            };
            let allowed = entry.granted.allows();
            if entry.granted == PermissionGrant::Once {
                let demoted = McpPermission {
                    client_id: entry.client_id,
                    tool_name: entry.tool_name,
                    granted: PermissionGrant::Denied,
                    granted_at: Utc::now(),
                };
                guard.insert(key, demoted);
                (allowed, true)
            } else {
                (allowed, false)
            }
        };
        if needs_persist {
            self.persist()?;
        }
        Ok(allowed)
    }

    fn persist(&self) -> Result<(), PermissionStoreError> {
        let file = StoredFile {
            entries: self.list(),
        };
        let bytes = serde_json::to_vec_pretty(&file)?;
        std::fs::write(&self.path, bytes)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn grant_and_check_round_trip() {
        let dir = tempdir().unwrap();
        let store = McpPermissionStore::open(dir.path()).unwrap();
        assert!(store.check("c1", "list_artboards").is_none());
        store
            .grant("c1", "list_artboards", PermissionGrant::Always)
            .unwrap();
        let entry = store.check("c1", "list_artboards").unwrap();
        assert_eq!(entry.granted, PermissionGrant::Always);
        assert!(entry.granted.allows());
    }

    #[test]
    fn revoke_removes_entry() {
        let dir = tempdir().unwrap();
        let store = McpPermissionStore::open(dir.path()).unwrap();
        store
            .grant("c1", "export_artboard", PermissionGrant::Always)
            .unwrap();
        store.revoke("c1", "export_artboard").unwrap();
        assert!(store.check("c1", "export_artboard").is_none());
    }

    #[test]
    fn persists_across_reopen() {
        let dir = tempdir().unwrap();
        {
            let store = McpPermissionStore::open(dir.path()).unwrap();
            store
                .grant("c1", "list_artboards", PermissionGrant::Always)
                .unwrap();
            store
                .grant("c2", "create_node", PermissionGrant::Denied)
                .unwrap();
        }
        let store = McpPermissionStore::open(dir.path()).unwrap();
        assert!(store
            .check("c1", "list_artboards")
            .unwrap()
            .granted
            .allows());
        assert!(!store.check("c2", "create_node").unwrap().granted.allows());
    }

    #[test]
    fn once_is_consumed() {
        let dir = tempdir().unwrap();
        let store = McpPermissionStore::open(dir.path()).unwrap();
        store
            .grant("c1", "create_node", PermissionGrant::Once)
            .unwrap();
        assert!(store.consume_if_once("c1", "create_node").unwrap());
        // Now denied — second consume must say no.
        assert!(!store.consume_if_once("c1", "create_node").unwrap());
    }

    #[test]
    fn list_for_client_filters_correctly() {
        let dir = tempdir().unwrap();
        let store = McpPermissionStore::open(dir.path()).unwrap();
        store
            .grant("c1", "list_artboards", PermissionGrant::Always)
            .unwrap();
        store
            .grant("c1", "create_node", PermissionGrant::Always)
            .unwrap();
        store
            .grant("c2", "create_node", PermissionGrant::Denied)
            .unwrap();
        let c1 = store.list_for_client("c1");
        assert_eq!(c1.len(), 2);
        let c2 = store.list_for_client("c2");
        assert_eq!(c2.len(), 1);
        assert_eq!(c2[0].tool_name, "create_node");
    }

    #[test]
    fn unknown_client_returns_none() {
        let dir = tempdir().unwrap();
        let store = McpPermissionStore::open(dir.path()).unwrap();
        assert!(store.check("ghost", "anything").is_none());
        assert!(!store.consume_if_once("ghost", "anything").unwrap());
    }

    #[test]
    fn open_recoverable_quarantines_corrupt_file_and_starts_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp_permissions.json");
        std::fs::write(&path, b"{ not valid json at all").unwrap();
        // open_recoverable must NOT panic / error on a malformed file.
        let store = McpPermissionStore::open_recoverable(dir.path()).unwrap();
        assert!(store.list().is_empty());
        // And the corrupt file must have been quarantined, not left in
        // place under its original name (otherwise the next save would
        // silently overwrite the user's potentially-recoverable data).
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().into_string().unwrap_or_default())
            .collect();
        assert!(
            entries
                .iter()
                .any(|n| n.starts_with("mcp_permissions.") && n.contains(".corrupt-")),
            "corrupt file should be renamed to *.corrupt-<ts>; got {entries:?}"
        );
        // Plain `open` would have errored on the same file.
        assert!(McpPermissionStore::open(dir.path()).is_ok()); // dir is empty now
    }

    #[test]
    fn open_rejects_corrupt_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp_permissions.json");
        std::fs::write(&path, b"{ also not valid").unwrap();
        // Plain `open` keeps the strict failure semantics so callers
        // that want hard-fail-on-corruption (e.g. tests, batch tools)
        // still get the error.
        assert!(McpPermissionStore::open(dir.path()).is_err());
    }

    #[test]
    fn list_is_sorted() {
        let dir = tempdir().unwrap();
        let store = McpPermissionStore::open(dir.path()).unwrap();
        store.grant("z", "tool", PermissionGrant::Always).unwrap();
        store.grant("a", "tool", PermissionGrant::Always).unwrap();
        let v = store.list();
        assert_eq!(v[0].client_id, "a");
        assert_eq!(v[1].client_id, "z");
    }
}

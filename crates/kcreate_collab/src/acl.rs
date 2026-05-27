//! Phase 7 (Task 21): per-document access-control list.
//!
//! Each `.kstudio/` project gains an `acl.json` file in its metadata
//! directory listing the peer public keys allowed to participate in
//! a collaborative session against that document. The session's host
//! consults the ACL during the Hello handshake: a peer whose public
//! key is not on the allow-list (and who isn't otherwise trusted
//! via the KChat community gate) is rejected with a typed reason.
//!
//! The ACL is structured around **peer public keys** (base64url
//! encoded, matching the wire format the protocol already uses on
//! [`crate::peer::PeerIdentity`]) rather than peer ids because the
//! UI surface that builds the ACL (`AccessControlPanel.tsx`) sees
//! peers via their KChat community-members roster, which carries
//! the public key. Peer ids are derived (BLAKE3 of the public key)
//! and can also be stored alongside for fast lookup.

use serde::{Deserialize, Serialize};

use crate::peer::{PeerId, PeerIdentity};

/// One entry in a project ACL: a peer's public key + the maximum
/// permission level the project owner has granted them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AclEntry {
    /// Base64url (no padding) of the peer's Ed25519 verifying key.
    /// Matches `PeerIdentity::public_key`.
    pub public_key: String,
    /// Optional human-readable label, surfaced by the renderer's
    /// `AccessControlPanel` so the owner can re-identify the entry
    /// without decoding the key.
    #[serde(default)]
    pub display_name: String,
    /// Permission level granted to this peer.
    pub permission: AclPermission,
}

/// Permission level granted to an ACL entry. Maps to
/// [`crate::session`]-level enforcement: editors may broadcast
/// `OperationBroadcast`, viewers may not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AclPermission {
    /// Full read-write access. May broadcast operations and claim
    /// locks.
    Editor,
    /// Read-only access. Receives operations + presence but the
    /// session host silently drops any outbound `OperationBroadcast`
    /// from this peer.
    Viewer,
}

/// The full ACL for one project.
///
/// `mode` controls how the ACL is consulted during Hello:
///
/// * [`AclMode::Open`] — community gate alone is enough; the ACL is
///   purely informational (the renderer may still show it but no
///   peer is rejected for being absent).
/// * [`AclMode::Enforce`] — a peer's public key must appear in
///   `entries` (or the local owner's key) to be admitted. Peers
///   present only via the community gate but absent from the ACL
///   are rejected at Hello-time with reason `"not authorized"`.
///
/// The default is [`AclMode::Open`] so legacy projects (no
/// `acl.json` file) keep working without forcing the user to
/// enumerate every peer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAcl {
    pub mode: AclMode,
    pub entries: Vec<AclEntry>,
}

impl Default for ProjectAcl {
    fn default() -> Self {
        Self {
            mode: AclMode::Open,
            entries: Vec::new(),
        }
    }
}

/// See [`ProjectAcl`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AclMode {
    Open,
    Enforce,
}

/// Decision returned by [`ProjectAcl::evaluate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AclDecision {
    /// The peer is admitted with the given permission. For
    /// [`AclMode::Open`] the permission defaults to
    /// [`AclPermission::Editor`].
    Allow(AclPermission),
    /// The peer must be rejected at Hello-time.
    Deny,
}

impl ProjectAcl {
    /// Decide whether the given peer is admissible. The host calls
    /// this from its Hello handler before sending the Welcome.
    #[must_use]
    pub fn evaluate(&self, identity: &PeerIdentity) -> AclDecision {
        match self.mode {
            AclMode::Open => AclDecision::Allow(AclPermission::Editor),
            AclMode::Enforce => self
                .entries
                .iter()
                .find(|e| e.public_key == identity.public_key)
                .map_or(AclDecision::Deny, |e| AclDecision::Allow(e.permission)),
        }
    }

    /// Insert or update an entry by public key. Returns the
    /// previous permission (if any).
    pub fn upsert(&mut self, entry: AclEntry) -> Option<AclPermission> {
        if let Some(slot) = self
            .entries
            .iter_mut()
            .find(|e| e.public_key == entry.public_key)
        {
            let prev = slot.permission;
            *slot = entry;
            Some(prev)
        } else {
            self.entries.push(entry);
            None
        }
    }

    /// Remove the entry with the given public key, if any. Returns
    /// the removed permission level.
    pub fn remove(&mut self, public_key: &str) -> Option<AclPermission> {
        let idx = self.entries.iter().position(|e| e.public_key == public_key)?;
        Some(self.entries.remove(idx).permission)
    }

    /// Return the entry for the given peer id, derived against
    /// every entry's public key (BLAKE3 hash). Useful when the
    /// caller only knows the peer id (e.g. an inbound message's
    /// `from` field).
    #[must_use]
    pub fn lookup_by_peer_id(&self, peer_id: &PeerId) -> Option<&AclEntry> {
        self.entries.iter().find(|e| {
            // Decode the public key and rederive the peer id.
            // Failures (bad encoding) just skip the entry.
            crate::peer::decode_public_key(&e.public_key)
                .ok()
                .is_some_and(|vk| PeerId::from_verifying_key(&vk) == *peer_id)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer::PeerKey;

    fn identity(seed: u8, name: &str) -> PeerIdentity {
        PeerKey::from_seed([seed; 32]).identity(name)
    }

    #[test]
    fn open_mode_allows_everyone_as_editor() {
        let acl = ProjectAcl::default();
        let ident = identity(1, "alice");
        assert_eq!(acl.evaluate(&ident), AclDecision::Allow(AclPermission::Editor));
    }

    #[test]
    fn enforce_mode_rejects_unknown_peers() {
        let acl = ProjectAcl {
            mode: AclMode::Enforce,
            entries: vec![],
        };
        assert_eq!(acl.evaluate(&identity(1, "alice")), AclDecision::Deny);
    }

    #[test]
    fn enforce_mode_grants_listed_permission() {
        let alice = identity(1, "alice");
        let bob = identity(2, "bob");
        let acl = ProjectAcl {
            mode: AclMode::Enforce,
            entries: vec![
                AclEntry {
                    public_key: alice.public_key.clone(),
                    display_name: "Alice".into(),
                    permission: AclPermission::Editor,
                },
                AclEntry {
                    public_key: bob.public_key.clone(),
                    display_name: "Bob".into(),
                    permission: AclPermission::Viewer,
                },
            ],
        };
        assert_eq!(acl.evaluate(&alice), AclDecision::Allow(AclPermission::Editor));
        assert_eq!(acl.evaluate(&bob), AclDecision::Allow(AclPermission::Viewer));
    }

    #[test]
    fn upsert_replaces_existing_entry() {
        let alice = identity(1, "alice");
        let mut acl = ProjectAcl {
            mode: AclMode::Enforce,
            entries: vec![AclEntry {
                public_key: alice.public_key.clone(),
                display_name: "Alice".into(),
                permission: AclPermission::Viewer,
            }],
        };
        let prev = acl.upsert(AclEntry {
            public_key: alice.public_key,
            display_name: "Alice (lead)".into(),
            permission: AclPermission::Editor,
        });
        assert_eq!(prev, Some(AclPermission::Viewer));
        assert_eq!(acl.entries.len(), 1);
        assert_eq!(acl.entries[0].permission, AclPermission::Editor);
        assert_eq!(acl.entries[0].display_name, "Alice (lead)");
    }

    #[test]
    fn remove_returns_previous_permission() {
        let alice = identity(1, "alice");
        let mut acl = ProjectAcl {
            mode: AclMode::Enforce,
            entries: vec![AclEntry {
                public_key: alice.public_key.clone(),
                display_name: "Alice".into(),
                permission: AclPermission::Editor,
            }],
        };
        assert_eq!(acl.remove(&alice.public_key), Some(AclPermission::Editor));
        assert!(acl.entries.is_empty());
        assert!(acl.remove(&alice.public_key).is_none());
    }

    #[test]
    fn lookup_by_peer_id_matches_derived_id() {
        let alice = identity(1, "alice");
        let acl = ProjectAcl {
            mode: AclMode::Enforce,
            entries: vec![AclEntry {
                public_key: alice.public_key.clone(),
                display_name: "Alice".into(),
                permission: AclPermission::Editor,
            }],
        };
        assert!(acl.lookup_by_peer_id(&alice.peer_id).is_some());
    }

    #[test]
    fn round_trip_serde() {
        let acl = ProjectAcl {
            mode: AclMode::Enforce,
            entries: vec![AclEntry {
                public_key: "ABC".into(),
                display_name: "Test".into(),
                permission: AclPermission::Editor,
            }],
        };
        let json = serde_json::to_string(&acl).unwrap();
        let back: ProjectAcl = serde_json::from_str(&json).unwrap();
        assert_eq!(back, acl);
    }
}

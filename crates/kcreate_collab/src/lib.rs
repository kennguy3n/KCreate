//! Phase 3 foundation: protocol types for optional LAN collaboration.
//!
//! KCreate is a **local-first** application. The editing path never
//! reaches over the network and the
//! `crates/kcreate_tests/tests/local_first.rs` sentinel enforces this
//! against a deny-list of HTTP / DNS / TLS / MQTT / Kafka / etc.
//! crates. This module is therefore kept **out of the editing-path
//! dependency tree** (see `editing_path_crates()` in that sentinel)
//! so that the Phase 3 LAN transport — when it lands — can pull in
//! mDNS / QUIC / Noise without violating the invariant.
//!
//! What this crate provides today:
//!
//! * [`peer`] — Ed25519-based peer identity + fingerprint computation,
//!   keyed off `kcreate_plugin`'s plugin-signing convention so the
//!   same trust UI can be reused.
//! * [`clock`] — a Lamport clock for total ordering of remote
//!   operations across peers.
//! * [`envelope`] — a generic signed-and-clocked envelope used to
//!   transport every payload between peers, with verification that
//!   refuses replays (`nonce`) and stale clocks.
//! * [`message`] — the closed set of payloads peers exchange today:
//!   session hello/welcome, operation broadcast, presence, heartbeat,
//!   goodbye.
//! * [`session`] — the stateful per-project session that tracks the
//!   local Lamport clock, known peers, and recently-seen nonces; the
//!   transport layer wraps this to actually send bytes.
//! * [`conflict`] — last-writer-wins conflict resolution for remote
//!   operations colliding with local edits, with deterministic peer-id
//!   tie-breaking and pluggable [`conflict::ConflictResolver`] trait
//!   for future strategies.
//! * [`crdt`] — operational-CRDT layer on top of `Operation`: classifies
//!   commands into delete / tree-move / property-update / create
//!   buckets, merges concurrent disjoint-key property edits into a
//!   single synthesised op, treats deletes as wins-over-edits, and
//!   resolves concurrent tree moves with Lamport + peer-id tiebreak.
//!   The transport layer asks the session to apply the resulting
//!   [`crdt::CrdtDecision`] atomically.
//!
//! What this crate intentionally does **not** provide:
//!
//! * Any network I/O. The Phase 3 transport (mDNS discovery, QUIC,
//!   Noise pre-shared-key handshake) is a separate crate that will
//!   layer on top of these types.
//! * Any UI surface. The Phase 3 collaboration panel will hang off the
//!   bridge layer like every other UI in KCreate.

pub mod acl;
pub mod clipboard;
pub mod clock;
pub mod conflict;
pub mod crdt;
pub mod envelope;
pub mod journal;
pub mod kchat;
pub mod message;
pub mod peer;
pub mod session;

pub use acl::{
    decrypt_acl_bytes, encrypt_acl_bytes, looks_like_encrypted_acl, AclCryptoError, AclDecision,
    AclEntry, AclMode, AclPermission, ProjectAcl, ACL_ENC_MAGIC, ACL_NONCE_LEN,
};
pub use clipboard::{
    decrypt_clipboard_payload, derive_x25519_from_ed25519_public, encrypt_clipboard_payload,
    ClipboardCryptoError, ClipboardPlaintext,
};
pub use clock::LamportClock;
pub use conflict::{ConflictDecision, ConflictResolver, LastWriterWinsResolver};
pub use crdt::{classify as classify_operation, CrdtDecision, CrdtResolver, OperationCategory};
pub use envelope::{CollabError, Envelope, EnvelopeBytes, SignedPayload, PROTOCOL_VERSION};
pub use journal::{
    JournalEntry, JournalError, JournalStore, MemoryJournalStore, OperationJournal, ResumeVector,
};
pub use kchat::{
    no_kchat_authority, BoundKChatGroupAuthority, KChatAuthError, KChatGroupAuthority,
    KChatGroupId, KChatMembership, NoKChatGroupAuthority, SharedKChatAuthority,
};
pub use message::{
    AnnotationBroadcastKind, AnnotationBroadcastPayload, ClipboardSharePayload, GoodbyeReason,
    HelloPayload, KeyRotationAckPayload, KeyRotationPayload, LockClaimPayload, LockReleasePayload,
    Message, OperationBroadcastPayload, PresencePayload, ResumeBundlePayload, ResumeRequestPayload,
    WelcomePayload, WelcomeStatus,
};
pub use peer::{decode_public_key, PeerFingerprint, PeerId, PeerIdentity, PeerKey};
pub use session::{ProjectSession, RateBudgetDecision, RateLimitKind, SessionConfig, SessionError};

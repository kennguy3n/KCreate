//! LAN transport for KCreate's Phase 3 collaboration protocol.
//!
//! The protocol types — peers, Lamport clocks, signed envelopes, the
//! per-project session state machine — live in [`kcreate_collab`]. This
//! crate provides the **only** networked layer in the workspace: a
//! QUIC-over-UDP transport for envelope delivery and an mDNS-SD layer
//! for zero-configuration peer discovery on the LAN.
//!
//! Why a separate crate
//!
//! The editing-path crates (`kcreate_core`, `kcreate_storage`,
//! `kcreate_vector`, `kcreate_export`, `kcreate_renderer`,
//! `kcreate_bridge`) are forbidden from pulling in any networking
//! library — the deny-list in
//! `crates/kcreate_tests/tests/local_first.rs` enforces this against
//! `reqwest`, `hyper`, `rustls`, `quinn-proto`, etc. Putting the
//! transport here, outside the editing-path tree, lets us depend on
//! quinn / rustls / mdns-sd / tokio without breaking that invariant.
//! The Electron host opts in by enabling the `collab` feature on
//! `kcreate_bridge`, which is the only consumer of this crate.
//!
//! Trust model
//!
//! Long-lived identity is an Ed25519 keypair held by
//! [`kcreate_collab::peer::PeerKey`]. Each [`LanCollabHost`] mints an
//! **ephemeral** self-signed TLS certificate at startup whose SHA-256
//! fingerprint is advertised in the mDNS TXT record. A custom
//! `rustls` server-cert verifier pins inbound connections to that
//! fingerprint, so an attacker cannot impersonate a discovered peer
//! by serving their own TLS cert. Authenticity of every message is
//! still proved at the application layer via the Ed25519 signature on
//! the [`kcreate_collab::envelope::Envelope`] — TLS only protects the
//! transport.
//!
//! What this crate ships today
//!
//! * [`host::LanCollabHost`] — owns a `quinn::Endpoint`, the
//!   [`kcreate_collab::session::ProjectSession`], and the mDNS
//!   responder. Exposes a small async API for the bridge:
//!   start / connect to discovered peer / broadcast / subscribe to
//!   inbound events / shutdown.
//! * [`discovery::PeerDiscovery`] — wraps `mdns-sd` so callers see a
//!   typed [`discovery::DiscoveredPeer`] stream. Used internally by
//!   the host; exported so the bridge can list peers in the UI.
//! * [`cert::CertBundle`] — ephemeral self-signed cert generation
//!   plus the cert-fingerprint-pinned verifier wired into rustls.
//! * [`InboundEvent`] — the union of `PeerJoined`, `PeerLeft`,
//!   `Message` the host broadcasts on its tokio `broadcast::Receiver`.
//!
//! What this crate intentionally does **not** do
//!
//! * Operational CRDT semantics. The wire format remains plain
//!   [`kcreate_collab::message::Message`] payloads inside signed
//!   envelopes; conflict resolution stays in
//!   [`kcreate_collab::conflict`].
//! * UI. The bridge surfaces session lifecycle (`session_start`,
//!   `session_join`, `session_peers`) and the renderer draws the
//!   presence overlay.

#![forbid(unsafe_code)]

pub mod cert;
pub mod discovery;
pub mod error;
pub mod host;
pub mod wire;

pub use cert::{cert_sha256_b64, CertBundle, PinnedFingerprintVerifier};
pub use discovery::{DiscoveredPeer, DiscoveryConfig, DiscoveryEvent, PeerDiscovery, SERVICE_TYPE};
pub use error::TransportError;
pub use host::{HostOptions, InboundEvent, LanCollabHost};
pub use wire::{decode_frame, encode_frame, MAX_FRAME_BYTES};

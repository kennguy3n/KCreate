//! The actor that owns the QUIC endpoint, the mDNS responder, and
//! the per-project [`kcreate_collab::ProjectSession`].
//!
//! Layering
//!
//! ```text
//!   PresencePanel / Editor (TS)        <- UI
//!         |
//!   bridge::collab (Rust, behind `collab` feature)
//!         |
//!   LanCollabHost (this module)         <- transport actor
//!     ├── ProjectSession                <- protocol state machine
//!     ├── PeerDiscovery (mDNS)          <- discovery
//!     ├── quinn::Endpoint (QUIC)        <- bytes
//!     └── tokio::broadcast<InboundEvent><- fan-out
//! ```
//!
//! All public API on the host is `async` and runs on a tokio runtime.
//! The bridge creates a dedicated tokio runtime in a worker thread
//! (the editing path itself stays single-threaded and sync) and
//! marshals method calls onto it via N-API.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use kcreate_collab::{
    no_kchat_authority, GoodbyeReason, HelloPayload, Message, OperationBroadcastPayload, PeerId,
    PeerIdentity, PeerKey, PresencePayload, ProjectSession, SessionConfig, SharedKChatAuthority,
    WelcomePayload, WelcomeStatus,
};
use parking_lot::RwLock;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{ClientConfig, Connection, Endpoint, ServerConfig, TransportConfig};
use rustls::pki_types::PrivateKeyDer;
use tokio::sync::{broadcast, mpsc, Mutex};
use uuid::Uuid;

use crate::cert::{CertBundle, PinnedFingerprintVerifier};
use crate::discovery::{DiscoveredPeer, DiscoveryConfig, DiscoveryEvent, PeerDiscovery};
use crate::error::TransportError;
use crate::wire::{encode_frame, MAX_FRAME_BYTES};

/// Default channel capacity for `subscribe()`. Same shape as the
/// discovery channel — large enough that an interactive UI never
/// drops events, small enough that a runaway producer is bounded.
const INBOUND_CHANNEL_CAPACITY: usize = 1024;

/// SNI hostname we pass to quinn on connect. The
/// [`PinnedFingerprintVerifier`] ignores hostnames entirely, but
/// rustls still wants a syntactically valid name.
const SNI_HOSTNAME: &str = "kcreate.local";

/// Configuration for starting a [`LanCollabHost`].
///
/// Not `Clone` because [`PeerKey`] intentionally isn't (its embedded
/// `SigningKey` shouldn't be duplicated by accident — there should
/// be exactly one in-memory holder of the secret per process).
#[derive(Debug)]
pub struct HostOptions {
    /// Long-lived peer signing key. **Not** serialised here — pass
    /// the key in directly, the host never persists it.
    pub local_key: PeerKey,
    /// Display name to render in remote peers' rosters.
    pub display_name: String,
    /// Project id the host is currently editing. Peers with a
    /// different project_id are rejected at Hello time.
    pub project_id: Uuid,
    /// UDP socket address to bind the QUIC endpoint to. `0.0.0.0:0`
    /// (or `[::]:0`) is the typical production choice; tests pin
    /// `127.0.0.1:0` to avoid LAN noise.
    pub bind_addr: SocketAddr,
    /// Whether to register an mDNS service so other peers can find
    /// us. Set false in tests where multicast is unreliable; peers
    /// can still be reached via [`LanCollabHost::dial_known_peer`].
    pub advertise_mdns: bool,
    /// Override the addresses advertised in mDNS A/AAAA records.
    /// `None` = let mdns-sd pick all routable interfaces. Tests use
    /// `[127.0.0.1]` to keep traffic on the loopback.
    pub advertise_addrs: Option<Vec<std::net::IpAddr>>,
    /// Optional override for the session config (replay window /
    /// peer cap). Defaults to [`SessionConfig::default`].
    pub session_config: SessionConfig,
    /// KChat group authority that gates every Hello/Welcome
    /// handshake. The default (`no_kchat_authority()`) fails closed
    /// — `start()` will refuse to construct the host so multiplayer
    /// stays locked. The bridge layer plugs in a real authority
    /// only once the user has signed into a KChat group.
    pub kchat_authority: SharedKChatAuthority,
}

impl HostOptions {
    /// Convenience builder for the typical "bind on loopback for
    /// tests" case.
    #[must_use]
    pub fn loopback(local_key: PeerKey, display_name: impl Into<String>, project_id: Uuid) -> Self {
        Self {
            local_key,
            display_name: display_name.into(),
            project_id,
            bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            advertise_mdns: false,
            advertise_addrs: Some(vec![std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)]),
            session_config: SessionConfig::default(),
            // Tests that don't care about the KChat gate use the
            // default-deny authority and immediately get a typed
            // `multiplayer locked` error from `start`. Tests that
            // *do* care construct the loopback options and then
            // overwrite `kchat_authority` with an
            // `InProcessKChatAuthority` before calling `start`.
            kchat_authority: no_kchat_authority(),
        }
    }
}

/// Events emitted on [`LanCollabHost::subscribe`]'s broadcast
/// channel. UI consumes these to drive the presence panel + cursor
/// overlay.
#[derive(Debug, Clone)]
pub enum InboundEvent {
    /// mDNS resolved a new peer. The UI shows a "Connect"
    /// confirmation prompt; if the user clicks through, the bridge
    /// calls [`LanCollabHost::dial_discovered_peer`].
    Discovered(DiscoveredPeer),
    /// A peer left the LAN (mDNS unregistration). Already-connected
    /// peers stay until the QUIC connection closes; this only signals
    /// removal from the discoverable roster.
    Undiscovered(PeerId),
    /// QUIC connection established (either inbound or outbound) and
    /// the peer has been trusted by the local session.
    PeerJoined(PeerIdentity),
    /// QUIC connection closed.
    PeerLeft(PeerId),
    /// A protocol message arrived from a connected peer. The
    /// envelope was already verified and the session's replay
    /// window updated; the bridge just needs to act on the payload.
    Message { from: PeerId, message: Box<Message> },
}

/// The transport actor. Cheap to clone — it's an `Arc` of inner
/// state.
#[derive(Clone)]
pub struct LanCollabHost {
    inner: Arc<HostInner>,
}

impl std::fmt::Debug for LanCollabHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LanCollabHost")
            .field("local_addr", &self.inner.local_addr)
            .field("local_identity", &self.inner.local_identity)
            .field("project_id", &self.inner.project_id)
            .field("connected_peer_count", &self.connected_peer_ids().len())
            .finish()
    }
}

struct HostInner {
    endpoint: Endpoint,
    local_addr: SocketAddr,
    local_identity: PeerIdentity,
    project_id: Uuid,
    /// The protocol state machine — locked because tokio tasks
    /// (accept loop, per-connection readers, broadcast) all touch
    /// it.
    session: Mutex<ProjectSession>,
    /// Map of currently-connected peers to the underlying QUIC
    /// `Connection`. The connection is `Clone` (it's internally an
    /// `Arc<Mutex<…>>`) so we can hand out copies for `broadcast`
    /// without taking the map lock for the entire send.
    peers: RwLock<HashMap<PeerId, PeerSlot>>,
    /// mDNS handle. Some when discovery was started, None for the
    /// "no LAN" smoke-test mode.
    discovery: RwLock<Option<Arc<PeerDiscovery>>>,
    /// Cert bundle held so we can re-advertise the fingerprint if
    /// mDNS is restarted, and so it stays alive for the QUIC
    /// server config.
    cert: CertBundle,
    /// Crypto provider shared with the client / verifier so they
    /// agree on supported algorithms.
    crypto_provider: Arc<rustls::crypto::CryptoProvider>,
    inbound_tx: broadcast::Sender<InboundEvent>,
    /// Hand to internal tasks for graceful shutdown. Each spawned
    /// task selects on the closed signal so `shutdown` reliably
    /// returns the runtime to idle.
    shutdown_tx: broadcast::Sender<()>,
    /// Application version string echoed in `Hello` for forensic
    /// display.
    app_version: String,
}

/// State per connected peer.
struct PeerSlot {
    identity: PeerIdentity,
    connection: Connection,
    /// Signals the per-connection reader loop to wind down.
    #[allow(dead_code)]
    drop_signal: mpsc::Sender<()>,
}

impl LanCollabHost {
    /// Start the transport: bind the QUIC endpoint, set up rustls,
    /// optionally start mDNS, and spawn the accept loop.
    pub async fn start(opts: HostOptions) -> Result<Self, TransportError> {
        // Make sure rustls' default crypto provider is installed.
        // ring is the one quinn's `rustls-ring` feature wires up. We
        // `try_install` because two transports in the same process
        // (e.g. integration tests) would otherwise fight over which
        // gets to install first.
        let crypto_provider = Arc::new(rustls::crypto::ring::default_provider());
        // Ignored if a provider is already installed for this
        // process — that's the desired idempotent behaviour.
        let _ = rustls::crypto::CryptoProvider::install_default(
            rustls::crypto::ring::default_provider(),
        );

        let cert = CertBundle::generate(vec![format!(
            "kcreate-peer-{}",
            opts.local_key.peer_id().as_str()
        )])?;

        let server_config = build_server_config(&cert)?;
        let endpoint =
            Endpoint::server(server_config, opts.bind_addr).map_err(|e| TransportError::Bind {
                addr: opts.bind_addr,
                source: e,
            })?;
        let local_addr = endpoint.local_addr().map_err(TransportError::Io)?;
        let local_identity = opts.local_key.identity(opts.display_name);

        // Seed the project session.
        let nonce_seed: [u8; 8] = blake3::hash(opts.project_id.as_bytes()).as_bytes()[..8]
            .try_into()
            .expect("blake3 hash is always >= 8 bytes");
        // Defence-in-depth gate at the transport boundary: if the
        // local KChat authority has no membership, refuse to start
        // the host outright. The session-level gate would catch the
        // first Hello / Welcome attempt, but failing here keeps the
        // QUIC endpoint from binding at all and gives the bridge a
        // clean "multiplayer locked" report instead of a stalled
        // handshake.
        if opts.kchat_authority.local_membership().is_none() {
            return Err(TransportError::UnsupportedAdvertisement(
                "multiplayer is locked: not signed into a KChat group".into(),
            ));
        }
        let session = ProjectSession::new_with_authority(
            opts.local_key,
            local_identity.display_name.clone(),
            opts.project_id,
            opts.session_config,
            nonce_seed,
            opts.kchat_authority,
        );

        let (inbound_tx, _inbound_rx) = broadcast::channel(INBOUND_CHANNEL_CAPACITY);
        let (shutdown_tx, _shutdown_rx) = broadcast::channel(8);

        let inner = Arc::new(HostInner {
            endpoint,
            local_addr,
            local_identity: local_identity.clone(),
            project_id: opts.project_id,
            session: Mutex::new(session),
            peers: RwLock::new(HashMap::new()),
            discovery: RwLock::new(None),
            cert,
            crypto_provider,
            inbound_tx,
            shutdown_tx,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
        });

        let host = Self { inner };

        // Optionally start mDNS.
        if opts.advertise_mdns || opts.advertise_addrs.is_some() {
            let cfg = DiscoveryConfig {
                identity: local_identity,
                project_id: opts.project_id,
                port: local_addr.port(),
                cert_fingerprint_b64: host.inner.cert.cert_fingerprint_b64(),
                advertise: opts.advertise_mdns,
                advertise_addrs: opts.advertise_addrs,
            };
            let discovery = Arc::new(PeerDiscovery::start(cfg)?);
            // Bridge discovery events into the inbound channel.
            let discovery_rx = discovery.subscribe();
            let inbound_tx = host.inner.inbound_tx.clone();
            let mut shutdown_rx = host.inner.shutdown_tx.subscribe();
            tokio::spawn(async move {
                let mut rx = discovery_rx;
                loop {
                    tokio::select! {
                        _ = shutdown_rx.recv() => break,
                        result = rx.recv() => {
                            match result {
                                Ok(DiscoveryEvent::Resolved(peer)) => {
                                    let _ = inbound_tx.send(InboundEvent::Discovered(peer));
                                }
                                Ok(DiscoveryEvent::Removed(peer_id)) => {
                                    let _ = inbound_tx.send(InboundEvent::Undiscovered(peer_id));
                                }
                                Err(broadcast::error::RecvError::Lagged(n)) => {
                                    log::warn!("discovery channel lagged by {n} events");
                                }
                                Err(broadcast::error::RecvError::Closed) => break,
                            }
                        }
                    }
                }
            });
            *host.inner.discovery.write() = Some(discovery);
        }

        // Accept loop.
        let accept_host = host.clone();
        let mut accept_shutdown = host.inner.shutdown_tx.subscribe();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = accept_shutdown.recv() => {
                        accept_host.inner.endpoint.close(0u32.into(), b"shutdown");
                        break;
                    }
                    maybe_incoming = accept_host.inner.endpoint.accept() => {
                        let Some(incoming) = maybe_incoming else {
                            break;
                        };
                        let host_for_conn = accept_host.clone();
                        tokio::spawn(async move {
                            match incoming.await {
                                Ok(conn) => {
                                    if let Err(e) = host_for_conn.handle_inbound_connection(conn).await {
                                        log::warn!("inbound connection ended: {e}");
                                    }
                                }
                                Err(e) => log::warn!("incoming.await failed: {e}"),
                            }
                        });
                    }
                }
            }
        });

        Ok(host)
    }

    /// The local QUIC socket address. Useful for tests and for
    /// generating "peer link" strings the user can paste into
    /// another instance.
    pub fn local_addr(&self) -> SocketAddr {
        self.inner.local_addr
    }

    /// The local peer's identity.
    pub fn local_identity(&self) -> PeerIdentity {
        self.inner.local_identity.clone()
    }

    /// The SHA-256 fingerprint of this host's TLS cert, base64-encoded
    /// the same way the mDNS TXT record advertises it.
    pub fn cert_fingerprint_b64(&self) -> String {
        self.inner.cert.cert_fingerprint_b64()
    }

    /// Project id the session is bound to.
    pub fn project_id(&self) -> Uuid {
        self.inner.project_id
    }

    /// Subscribe to inbound transport events. Each new subscriber
    /// only sees events from this point onward.
    pub fn subscribe(&self) -> broadcast::Receiver<InboundEvent> {
        self.inner.inbound_tx.subscribe()
    }

    /// Snapshot of the connected peers' identities. Cheap clone.
    pub fn connected_peers(&self) -> Vec<PeerIdentity> {
        self.inner
            .peers
            .read()
            .values()
            .map(|p| p.identity.clone())
            .collect()
    }

    /// Snapshot of just the connected peer ids.
    pub fn connected_peer_ids(&self) -> Vec<PeerId> {
        self.inner.peers.read().keys().cloned().collect()
    }

    /// Snapshot of the currently-resolved peers from mDNS. Empty if
    /// discovery isn't running.
    pub fn discovered_peers(&self) -> Vec<PeerId> {
        self.inner
            .discovery
            .read()
            .as_ref()
            .map(|d| d.resolved_peer_ids())
            .unwrap_or_default()
    }

    /// Dial a peer discovered via mDNS, completing the TLS handshake
    /// (with fingerprint pinning), the Hello/Welcome handshake, and
    /// adding the peer to the session roster.
    ///
    /// Idempotent: calling this for an already-connected peer just
    /// returns the existing identity.
    pub async fn dial_discovered_peer(
        &self,
        peer: &DiscoveredPeer,
    ) -> Result<PeerIdentity, TransportError> {
        if peer.project_id != self.inner.project_id {
            return Err(TransportError::UnsupportedAdvertisement(format!(
                "discovered peer is on project {}, local session is on {}",
                peer.project_id, self.inner.project_id
            )));
        }
        if let Some(existing) = self.inner.peers.read().get(&peer.peer_id) {
            return Ok(existing.identity.clone());
        }
        self.dial(
            peer.identity.clone(),
            peer.socket_addr,
            peer.cert_fingerprint,
        )
        .await
    }

    /// Dial a peer whose connection details came in out-of-band (a
    /// pasted "peer link" or an explicit "connect to address" UI
    /// flow). Bypasses mDNS but otherwise behaves identically.
    pub async fn dial_known_peer(
        &self,
        identity: PeerIdentity,
        socket_addr: SocketAddr,
        cert_fingerprint: [u8; 32],
    ) -> Result<PeerIdentity, TransportError> {
        if let Some(existing) = self.inner.peers.read().get(&identity.peer_id) {
            return Ok(existing.identity.clone());
        }
        self.dial(identity, socket_addr, cert_fingerprint).await
    }

    /// Broadcast an envelope to every connected peer. Each peer
    /// receives the message on a fresh bidi stream so head-of-line
    /// blocking can't make a slow peer stall the others.
    pub async fn broadcast(&self, message: Message) -> Result<(), TransportError> {
        let frame = {
            let mut session = self.inner.session.lock().await;
            let json = session.seal_message(message)?;
            encode_frame(json.as_bytes())?
        };
        let connections: Vec<Connection> = self
            .inner
            .peers
            .read()
            .values()
            .map(|p| p.connection.clone())
            .collect();
        for conn in connections {
            let frame = frame.clone();
            // Per-peer write happens on a detached task so a slow
            // peer can't hold up the broadcast for everyone else.
            tokio::spawn(async move {
                if let Err(e) = write_frame(&conn, &frame).await {
                    log::warn!("broadcast write to {} failed: {e}", conn.remote_address());
                }
            });
        }
        Ok(())
    }

    /// Send a direct (non-broadcast) message to a single peer.
    pub async fn send_to(&self, peer_id: &PeerId, message: Message) -> Result<(), TransportError> {
        let conn = self
            .inner
            .peers
            .read()
            .get(peer_id)
            .map(|slot| slot.connection.clone())
            .ok_or_else(|| {
                TransportError::Quic(format!("peer {} is not connected", peer_id.as_str()))
            })?;
        let frame = {
            let mut session = self.inner.session.lock().await;
            let json = session.seal_message(message)?;
            encode_frame(json.as_bytes())?
        };
        write_frame(&conn, &frame).await
    }

    /// Sugar: build + broadcast a `Presence` payload.
    pub async fn broadcast_presence(&self, payload: PresencePayload) -> Result<(), TransportError> {
        self.broadcast(Message::Presence(payload)).await
    }

    /// Sugar: build + broadcast a batch of operations.
    pub async fn broadcast_operations(
        &self,
        payload: OperationBroadcastPayload,
    ) -> Result<(), TransportError> {
        self.broadcast(Message::OperationBroadcast(payload)).await
    }

    /// Block 8: sugar — fan out a `LockClaim` to every connected
    /// peer. Receivers run the soft-lock contract on their side.
    pub async fn broadcast_lock_claim(
        &self,
        payload: kcreate_collab::LockClaimPayload,
    ) -> Result<(), TransportError> {
        self.broadcast(Message::LockClaim(payload)).await
    }

    /// Block 8: sugar — fan out a `LockRelease`.
    pub async fn broadcast_lock_release(
        &self,
        payload: kcreate_collab::LockReleasePayload,
    ) -> Result<(), TransportError> {
        self.broadcast(Message::LockRelease(payload)).await
    }

    /// Send a graceful `Goodbye` to every peer and close the QUIC
    /// endpoint. Idempotent; calling twice is a no-op.
    pub async fn shutdown(self) {
        // Best-effort goodbye.
        let _ = self
            .broadcast(Message::Goodbye(GoodbyeReason::Normal))
            .await;
        // Tell internal tasks to stop. Receivers that aren't
        // listening drop the message — that's fine because shutdown
        // happens on the way out.
        let _ = self.inner.shutdown_tx.send(());
        // Drop the mdns handle (its `Drop` impl unregisters).
        self.inner.discovery.write().take();
        // Wait for the endpoint to flush.
        self.inner.endpoint.close(0u32.into(), b"shutdown");
        self.inner.endpoint.wait_idle().await;
    }

    // ---------- private helpers ----------

    async fn dial(
        &self,
        identity: PeerIdentity,
        socket_addr: SocketAddr,
        cert_fingerprint: [u8; 32],
    ) -> Result<PeerIdentity, TransportError> {
        let verifier =
            PinnedFingerprintVerifier::new(cert_fingerprint, self.inner.crypto_provider.clone());
        let mut client_tls =
            rustls::ClientConfig::builder_with_provider(self.inner.crypto_provider.clone())
                .with_safe_default_protocol_versions()
                .map_err(|e| TransportError::Tls(format!("client builder: {e}")))?
                .dangerous()
                .with_custom_certificate_verifier(verifier)
                .with_no_client_auth();
        // ALPN gives middleboxes something to grep for and lets us
        // version the application protocol independently of QUIC.
        client_tls.alpn_protocols = vec![b"kcreate-collab/1".to_vec()];
        let client_config = ClientConfig::new(Arc::new(
            QuicClientConfig::try_from(client_tls)
                .map_err(|e| TransportError::Tls(format!("quinn client cfg: {e}")))?,
        ));

        let connecting =
            self.inner
                .endpoint
                .connect_with(client_config, socket_addr, SNI_HOSTNAME)?;
        let connection = connecting.await?;
        self.install_connection(identity, connection, /* we_dialed = */ true)
            .await
    }

    async fn handle_inbound_connection(
        &self,
        connection: Connection,
    ) -> Result<(), TransportError> {
        // The peer that dialed us sends a `Hello` envelope on the
        // first bidi stream. Read it, trust them on the spot if the
        // project matches, reply with `Welcome`, then add them to
        // the roster.
        let (mut send, mut recv) = connection.accept_bi().await?;
        let frame_bytes = read_frame(&mut recv).await?;
        let env: kcreate_collab::Envelope<Message> = serde_json::from_slice(&frame_bytes)
            .map_err(|e| TransportError::Malformed(e.to_string()))?;
        // `Envelope::payload` is a `pub` field — we deliberately
        // *peek* before verification so we can extract the joiner's
        // identity (which carries the public key) and trust it
        // before calling `ingest_envelope`, which is what actually
        // verifies the Ed25519 signature. A maliciously-formed Hello
        // that lies about its public key is rejected one of two
        // ways: (1) `trust_peer` rederives the peer id from the
        // public key and rejects an id mismatch, (2) `ingest_envelope`
        // verifies the signature with that key — if the key isn't
        // the real signer, verification fails.
        let peer_identity = match &env.payload {
            Message::Hello(p) => p.identity.clone(),
            other => {
                return Err(TransportError::UnsupportedAdvertisement(format!(
                    "first message must be Hello, got {}",
                    message_kind(other)
                )));
            }
        };
        // Trust + ingest in one shot.
        {
            let mut session = self.inner.session.lock().await;
            session.trust_peer(peer_identity.clone())?;
            let message = session.ingest_envelope(env)?;
            match message {
                Message::Hello(payload) => {
                    if payload.project_id != self.inner.project_id {
                        let welcome = WelcomePayload {
                            status: WelcomeStatus::Rejected,
                            host_identity: self.inner.local_identity.clone(),
                            host_clock: session.clock(),
                            reject_reason: format!(
                                "host is on project {}, joiner asked for {}",
                                self.inner.project_id, payload.project_id
                            ),
                            // Rejected welcomes deliberately do NOT carry an
                            // attestation; `seal_message` only stamps accept paths.
                            kchat_attestation: None,
                        };
                        let frame = {
                            let json = session.seal_message(Message::Welcome(welcome))?;
                            encode_frame(json.as_bytes())?
                        };
                        send.write_all(&frame).await?;
                        let _ = send.finish();
                        session.forget_peer(&peer_identity.peer_id);
                        return Err(TransportError::UnsupportedAdvertisement(
                            "joiner project_id mismatch".into(),
                        ));
                    }
                }
                _ => unreachable!("kind already checked"),
            }
            // Accept. The session's `seal_message` will stamp in the
            // host's KChat attestation, so the inline construction
            // leaves it `None`.
            let welcome = WelcomePayload {
                status: WelcomeStatus::Accepted,
                host_identity: self.inner.local_identity.clone(),
                host_clock: session.clock(),
                reject_reason: String::new(),
                kchat_attestation: None,
            };
            let frame = {
                let json = session.seal_message(Message::Welcome(welcome))?;
                encode_frame(json.as_bytes())?
            };
            send.write_all(&frame).await?;
            let _ = send.finish();
        }
        self.install_connection(peer_identity, connection, /* we_dialed = */ false)
            .await?;
        Ok(())
    }

    async fn install_connection(
        &self,
        identity: PeerIdentity,
        connection: Connection,
        we_dialed: bool,
    ) -> Result<PeerIdentity, TransportError> {
        let peer_id = identity.peer_id.clone();

        // If we're the dialer we still need to send Hello and wait
        // for Welcome before considering the peer connected.
        if we_dialed {
            let (mut send, mut recv) = connection.open_bi().await?;
            let hello = HelloPayload {
                identity: self.inner.local_identity.clone(),
                project_id: self.inner.project_id,
                app_version: self.inner.app_version.clone(),
                // Stamped in by `seal_message` from the local authority.
                kchat_attestation: None,
            };
            let frame = {
                let mut session = self.inner.session.lock().await;
                // We have to trust the peer before ingesting any
                // response from them. Use the identity we were told
                // about (via mDNS or out-of-band).
                session.trust_peer(identity.clone())?;
                let json = session.seal_message(Message::Hello(hello))?;
                encode_frame(json.as_bytes())?
            };
            send.write_all(&frame).await?;
            let _ = send.finish();
            let welcome_bytes = read_frame(&mut recv).await?;
            let env: kcreate_collab::Envelope<Message> = serde_json::from_slice(&welcome_bytes)
                .map_err(|e| TransportError::Malformed(e.to_string()))?;
            let mut session = self.inner.session.lock().await;
            let welcome = session.ingest_envelope(env)?;
            match welcome {
                Message::Welcome(payload) => match payload.status {
                    WelcomeStatus::Accepted => {
                        // Observe host clock so subsequent local
                        // sends order after it.
                        let _ = session.clock();
                    }
                    WelcomeStatus::Rejected => {
                        session.forget_peer(&identity.peer_id);
                        return Err(TransportError::UnsupportedAdvertisement(format!(
                            "remote rejected: {}",
                            payload.reject_reason
                        )));
                    }
                },
                other => {
                    session.forget_peer(&identity.peer_id);
                    return Err(TransportError::UnsupportedAdvertisement(format!(
                        "expected Welcome after Hello, got {}",
                        message_kind(&other)
                    )));
                }
            }
        }

        let (drop_tx, drop_rx) = mpsc::channel(1);
        {
            let mut peers = self.inner.peers.write();
            peers.insert(
                peer_id.clone(),
                PeerSlot {
                    identity: identity.clone(),
                    connection: connection.clone(),
                    drop_signal: drop_tx,
                },
            );
        }
        let _ = self
            .inner
            .inbound_tx
            .send(InboundEvent::PeerJoined(identity.clone()));

        // Spawn the per-connection reader.
        let host = self.clone();
        let conn_for_task = connection.clone();
        let peer_id_for_task = peer_id.clone();
        tokio::spawn(async move {
            host.connection_reader_loop(peer_id_for_task, conn_for_task, drop_rx)
                .await;
        });

        Ok(identity)
    }

    async fn connection_reader_loop(
        &self,
        peer_id: PeerId,
        connection: Connection,
        mut drop_rx: mpsc::Receiver<()>,
    ) {
        let mut shutdown_rx = self.inner.shutdown_tx.subscribe();
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => break,
                _ = drop_rx.recv() => break,
                bi = connection.accept_bi() => {
                    match bi {
                        Ok((_send, mut recv)) => {
                            match read_frame(&mut recv).await {
                                Ok(bytes) => {
                                    if let Err(e) = self.dispatch_inbound(&peer_id, &bytes).await {
                                        log::warn!(
                                            "drop frame from {}: {e}",
                                            peer_id.as_str()
                                        );
                                    }
                                }
                                Err(e) => {
                                    log::debug!(
                                        "read frame from {} ended: {e}",
                                        peer_id.as_str()
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            log::debug!(
                                "accept_bi from {} ended: {e}",
                                peer_id.as_str()
                            );
                            break;
                        }
                    }
                }
            }
        }
        // Connection closed — drop the peer.
        {
            self.inner.peers.write().remove(&peer_id);
        }
        let mut session = self.inner.session.lock().await;
        session.forget_peer(&peer_id);
        drop(session);
        let _ = self.inner.inbound_tx.send(InboundEvent::PeerLeft(peer_id));
    }

    async fn dispatch_inbound(&self, from: &PeerId, bytes: &[u8]) -> Result<(), TransportError> {
        let env: kcreate_collab::Envelope<Message> =
            serde_json::from_slice(bytes).map_err(|e| TransportError::Malformed(e.to_string()))?;
        let mut session = self.inner.session.lock().await;
        let message = session.ingest_envelope(env)?;
        drop(session);
        let _ = self.inner.inbound_tx.send(InboundEvent::Message {
            from: from.clone(),
            message: Box::new(message),
        });
        Ok(())
    }
}

fn build_server_config(cert: &CertBundle) -> Result<ServerConfig, TransportError> {
    let cert_chain = vec![cert.cert_der.clone()];
    let key_der: PrivateKeyDer<'static> = PrivateKeyDer::Pkcs8(cert.key_pkcs8.clone_key());
    let mut server_tls = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| TransportError::Tls(format!("server builder: {e}")))?
    .with_no_client_auth()
    .with_single_cert(cert_chain, key_der)
    .map_err(|e| TransportError::Tls(format!("server tls: {e}")))?;
    server_tls.alpn_protocols = vec![b"kcreate-collab/1".to_vec()];

    let mut server_config = ServerConfig::with_crypto(Arc::new(
        QuicServerConfig::try_from(server_tls)
            .map_err(|e| TransportError::Tls(format!("quinn server cfg: {e}")))?,
    ));
    // Reasonable transport tuning for an interactive LAN session.
    let mut transport = TransportConfig::default();
    transport.max_concurrent_bidi_streams(256u32.into());
    transport.max_concurrent_uni_streams(0u32.into());
    transport.keep_alive_interval(Some(std::time::Duration::from_secs(10)));
    transport.max_idle_timeout(Some(
        std::time::Duration::from_secs(30)
            .try_into()
            .map_err(|e| TransportError::Tls(format!("idle timeout: {e}")))?,
    ));
    server_config.transport_config(Arc::new(transport));
    Ok(server_config)
}

async fn read_frame(recv: &mut quinn::RecvStream) -> Result<Vec<u8>, TransportError> {
    let mut header = [0u8; std::mem::size_of::<u32>()];
    recv.read_exact(&mut header).await?;
    let len = u32::from_be_bytes(header) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(TransportError::FrameTooLarge {
            size: len,
            max: MAX_FRAME_BYTES,
        });
    }
    let mut buf = vec![0u8; len];
    recv.read_exact(&mut buf).await?;
    Ok(buf)
}

async fn write_frame(connection: &Connection, frame: &[u8]) -> Result<(), TransportError> {
    let (mut send, _recv) = connection.open_bi().await?;
    send.write_all(frame).await?;
    let _ = send.finish();
    Ok(())
}

fn message_kind(message: &Message) -> &'static str {
    match message {
        Message::Hello(_) => "Hello",
        Message::Welcome(_) => "Welcome",
        Message::OperationBroadcast(_) => "OperationBroadcast",
        Message::Presence(_) => "Presence",
        Message::Heartbeat => "Heartbeat",
        Message::Goodbye(_) => "Goodbye",
        Message::ResumeRequest(_) => "ResumeRequest",
        Message::ResumeBundle(_) => "ResumeBundle",
        Message::LockClaim(_) => "LockClaim",
        Message::LockRelease(_) => "LockRelease",
    }
}

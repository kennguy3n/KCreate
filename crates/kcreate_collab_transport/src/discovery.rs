//! mDNS service discovery for KCreate peers on the LAN.
//!
//! Each [`crate::host::LanCollabHost`] advertises one service record:
//!
//! ```text
//! Service type:  _kcreate-collab._udp.local.
//! Instance name: <peer_id>.<service_type>
//! Port:          UDP port the QUIC endpoint is bound to
//! TXT properties:
//!   v   = "1"                   protocol version (matches PROTOCOL_VERSION)
//!   pk  = <pub_key>             base64url (no pad) of the 32-byte
//!                               Ed25519 verifying key; the canonical
//!                               peer_id + fingerprint are derived from it
//!   cf  = <cert_sha256>         base64 of the leaf cert DER's SHA-256
//!   pj  = <project_id>          uuid as a hyphenated string
//!   nm  = <display_name>        UTF-8, up to 64 chars after PeerIdentity trim
//! ```
//!
//! The peer's Ed25519 public key is the only key field on the wire;
//! [`PeerId`] and [`PeerFingerprint`] are recomputed from it at parse
//! time, which doubles as a sanity check (a malicious peer cannot
//! advertise pid=X but use pubkey=Y since pid is *derived* from
//! pubkey here, not parsed independently).

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::VerifyingKey;
use kcreate_collab::{PeerFingerprint, PeerId, PeerIdentity};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use parking_lot::RwLock;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::cert::cert_sha256_from_b64;
use crate::error::TransportError;

/// The fixed mDNS service type KCreate peers advertise on. The choice
/// of `_udp` reflects the QUIC transport — even though the QUIC
/// stream API resembles TCP, the underlying packets are UDP.
pub const SERVICE_TYPE: &str = "_kcreate-collab._udp.local.";

/// Default channel capacity for the discovery event stream. Each
/// `DiscoveryEvent` is small (≤ a few hundred bytes); 1024 is plenty
/// for an interactive UI even if mDNS goes wild on a busy LAN.
const EVENT_CHANNEL_CAPACITY: usize = 1024;

/// Wire-format protocol version this transport speaks. Bump in
/// lockstep with [`kcreate_collab::PROTOCOL_VERSION`] when the
/// envelope format or message variants change.
pub const TRANSPORT_PROTOCOL_VERSION: u32 = kcreate_collab::PROTOCOL_VERSION;

/// A peer we've seen on the LAN. Holds enough information for the
/// UI to render a roster entry and for the transport to dial.
#[derive(Debug, Clone)]
pub struct DiscoveredPeer {
    /// Public identity ready to feed into
    /// [`kcreate_collab::ProjectSession::trust_peer`] after the user
    /// approves the fingerprint.
    pub identity: PeerIdentity,
    /// Convenience copy of [`PeerIdentity::peer_id`] for the
    /// roster maps in the host actor.
    pub peer_id: PeerId,
    /// Convenience copy of the human-presentable fingerprint
    /// derived from `identity.verifying_key()`.
    pub fingerprint: PeerFingerprint,
    /// Project the peer is part of. Must match our local project id
    /// before we attempt to connect.
    pub project_id: Uuid,
    /// QUIC endpoint to dial.
    pub socket_addr: SocketAddr,
    /// SHA-256 of the peer's TLS leaf cert DER. Used by
    /// [`crate::cert::PinnedFingerprintVerifier`] when we dial.
    pub cert_fingerprint: [u8; 32],
    /// Protocol version the peer speaks; today this always equals
    /// [`TRANSPORT_PROTOCOL_VERSION`] because we reject any other in
    /// [`parse_discovered`].
    pub proto_version: u32,
}

/// Discovery events surfaced to the host actor. `Resolved` is what
/// the UI actually cares about; the others are for telemetry /
/// debugging.
#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    /// A peer has been fully resolved (TXT + address). The only
    /// variant the UI shows directly.
    Resolved(DiscoveredPeer),
    /// A previously-resolved peer was removed (left the LAN).
    Removed(PeerId),
}

/// Handle to a running mDNS browser + responder pair. Dropping the
/// handle shuts down both.
pub struct PeerDiscovery {
    daemon: Arc<ServiceDaemon>,
    events_tx: broadcast::Sender<DiscoveryEvent>,
    registered_fullname: Arc<RwLock<Option<String>>>,
    own_peer_id: PeerId,
    /// Resolved peers indexed by mDNS instance fullname so the
    /// removal handler can map back to a `PeerId`. The map is
    /// guarded by `RwLock` because the mDNS reader runs on its own
    /// thread.
    resolved: Arc<RwLock<HashMap<String, PeerId>>>,
}

impl std::fmt::Debug for PeerDiscovery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerDiscovery")
            .field("own_peer_id", &self.own_peer_id)
            .field(
                "registered_fullname",
                &self.registered_fullname.read().clone(),
            )
            .field("resolved_count", &self.resolved.read().len())
            .finish()
    }
}

/// Configuration for [`PeerDiscovery::start`].
#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    /// Our identity, advertised over mDNS so peers can render us in
    /// their roster.
    pub identity: PeerIdentity,
    pub project_id: Uuid,
    /// Local QUIC port the peer is listening on. Advertised in the
    /// SRV record.
    pub port: u16,
    /// Base64 of the SHA-256 of the leaf TLS cert. Advertised in TXT
    /// so dialers can pin the connection.
    pub cert_fingerprint_b64: String,
    /// Whether to actually register an advertised service. Some
    /// callers (e.g. headless tests) only want to browse and not
    /// pollute the LAN; set this to `false` in that case.
    pub advertise: bool,
    /// Optional override of the bind addresses to advertise. When
    /// `None`, mDNS will pick up all routable IPv4/IPv6 interfaces
    /// (`enable_addr_auto`). Tests on the loopback use this to
    /// override to `127.0.0.1` so the daemon doesn't fail to bind.
    pub advertise_addrs: Option<Vec<IpAddr>>,
}

impl PeerDiscovery {
    /// Start the mDNS daemon, register our service (if
    /// `cfg.advertise` is true), and start a background browser.
    /// The returned handle's [`subscribe`](Self::subscribe) channel
    /// emits a [`DiscoveryEvent`] for every peer the daemon resolves.
    pub fn start(cfg: DiscoveryConfig) -> Result<Self, TransportError> {
        let daemon =
            ServiceDaemon::new().map_err(|e| TransportError::Mdns(format!("daemon new: {e}")))?;
        let (events_tx, _events_rx) = broadcast::channel(EVENT_CHANNEL_CAPACITY);

        // Build the TXT properties dict. mDNS limits each `key=value`
        // pair to 255 bytes, which is comfortable for all of our
        // values.
        let mut txt: HashMap<String, String> = HashMap::new();
        txt.insert("v".into(), TRANSPORT_PROTOCOL_VERSION.to_string());
        txt.insert("pk".into(), cfg.identity.public_key.clone());
        txt.insert("cf".into(), cfg.cert_fingerprint_b64.clone());
        txt.insert("pj".into(), cfg.project_id.to_string());
        txt.insert("nm".into(), cfg.identity.display_name.clone());

        let instance_name = encode_instance_name(&cfg.identity.peer_id);

        let registered_fullname = Arc::new(RwLock::new(None));
        if cfg.advertise {
            let host_name = format!("{instance_name}.local.");
            let info = if let Some(addrs) = cfg.advertise_addrs.clone() {
                let ips: Vec<IpAddr> = addrs;
                ServiceInfo::new(
                    SERVICE_TYPE,
                    &instance_name,
                    &host_name,
                    &ips[..],
                    cfg.port,
                    Some(txt.clone()),
                )
                .map_err(|e| TransportError::Mdns(format!("service info: {e}")))?
            } else {
                let empty_ips: &[IpAddr] = &[];
                ServiceInfo::new(
                    SERVICE_TYPE,
                    &instance_name,
                    &host_name,
                    empty_ips,
                    cfg.port,
                    Some(txt.clone()),
                )
                .map_err(|e| TransportError::Mdns(format!("service info: {e}")))?
                .enable_addr_auto()
            };
            let fullname = info.get_fullname().to_string();
            daemon
                .register(info)
                .map_err(|e| TransportError::Mdns(format!("register: {e}")))?;
            *registered_fullname.write() = Some(fullname);
        }

        let browser = daemon
            .browse(SERVICE_TYPE)
            .map_err(|e| TransportError::Mdns(format!("browse: {e}")))?;
        let daemon_arc = Arc::new(daemon);
        let resolved = Arc::new(RwLock::new(HashMap::new()));
        let events_tx_clone = events_tx.clone();
        let registered_fullname_clone = registered_fullname.clone();
        // We need two `PeerId` handles: one to keep on the
        // `PeerDiscovery` struct (for `resolved_peer_ids` and
        // friends) and one that moves into the spawned bridge
        // thread so the closure can filter out our own
        // advertisement. The bridge thread is detached, so we have
        // to move-clone rather than borrow.
        let own_peer_id_for_struct = cfg.identity.peer_id.clone();
        let own_peer_id_for_thread = cfg.identity.peer_id;
        let resolved_clone = resolved.clone();

        // The mDNS daemon delivers events on its own flume channel
        // (sync, not tokio). We bridge that into our tokio broadcast
        // channel on a dedicated thread so the bridge / UI side can
        // consume events purely via tokio.
        std::thread::Builder::new()
            .name("kcreate-mdns-bridge".into())
            .spawn(move || {
                for event in &browser {
                    match event {
                        ServiceEvent::ServiceResolved(info) => {
                            // Skip our own service.
                            if let Some(own) = registered_fullname_clone.read().as_deref() {
                                if own == info.get_fullname() {
                                    continue;
                                }
                            }
                            match parse_discovered(&info) {
                                Ok(peer) => {
                                    if peer.peer_id == own_peer_id_for_thread {
                                        continue;
                                    }
                                    resolved_clone.write().insert(
                                        info.get_fullname().to_string(),
                                        peer.peer_id.clone(),
                                    );
                                    let _ = events_tx_clone.send(DiscoveryEvent::Resolved(peer));
                                }
                                Err(e) => {
                                    log::debug!(
                                        "ignoring unparseable kcreate peer advertisement \
                                         from {}: {}",
                                        info.get_fullname(),
                                        e
                                    );
                                }
                            }
                        }
                        ServiceEvent::ServiceRemoved(_ty, fullname) => {
                            // Bind the lock-write-and-remove result
                            // to a temporary so clippy's
                            // significant-drop-in-scrutinee lint is
                            // satisfied (the parking_lot write guard
                            // is otherwise held across the
                            // `let Some(...)` arm and could deadlock
                            // future readers in the same task).
                            let removed = resolved_clone.write().remove(&fullname);
                            if let Some(peer_id) = removed {
                                let _ = events_tx_clone.send(DiscoveryEvent::Removed(peer_id));
                            }
                        }
                        // ServiceFound / SearchStarted / SearchStopped
                        // are informational; we surface only resolved
                        // peers to higher layers.
                        _ => {}
                    }
                }
            })
            .map_err(|e| TransportError::Mdns(format!("spawn mdns bridge: {e}")))?;

        Ok(Self {
            daemon: daemon_arc,
            events_tx,
            registered_fullname,
            own_peer_id: own_peer_id_for_struct,
            resolved,
        })
    }

    /// Subscribe to the discovery event stream. Each new subscriber
    /// only sees events from this point onward.
    pub fn subscribe(&self) -> broadcast::Receiver<DiscoveryEvent> {
        self.events_tx.subscribe()
    }

    /// The mDNS instance fullname this peer was advertised under, if
    /// any. `None` when `advertise` was disabled.
    #[must_use]
    pub fn advertised_fullname(&self) -> Option<String> {
        self.registered_fullname.read().clone()
    }

    /// Snapshot of currently-resolved peers' IDs.
    pub fn resolved_peer_ids(&self) -> Vec<PeerId> {
        self.resolved.read().values().cloned().collect()
    }

    /// Best-effort shutdown: unregister the advertised service and
    /// stop the daemon. Errors are logged but not returned because
    /// we still want to drop the handle on transport shutdown.
    pub fn shutdown(&self) {
        // Drop the read guard before calling into `unregister`;
        // otherwise the parking_lot RwLockReadGuard would live for
        // the body of the `if let` arm and could starve writers.
        let fullname = self.registered_fullname.read().clone();
        if let Some(fullname) = fullname {
            let _ = self.daemon.unregister(&fullname);
        }
        let _ = self.daemon.shutdown();
    }
}

impl Drop for PeerDiscovery {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Parse a resolved mDNS service into a [`DiscoveredPeer`]. Returns
/// an error if any required TXT field is missing or the cert
/// fingerprint / public key is malformed.
fn parse_discovered(info: &ServiceInfo) -> Result<DiscoveredPeer, TransportError> {
    fn txt<'a>(info: &'a ServiceInfo, key: &str) -> Result<&'a str, TransportError> {
        info.get_property_val_str(key).ok_or_else(|| {
            TransportError::UnsupportedAdvertisement(format!(
                "missing TXT property '{key}' on {}",
                info.get_fullname()
            ))
        })
    }

    let v_str = txt(info, "v")?;
    let proto_version: u32 = v_str.parse().map_err(|_| {
        TransportError::UnsupportedAdvertisement(format!("invalid protocol version '{v_str}'"))
    })?;
    if proto_version != TRANSPORT_PROTOCOL_VERSION {
        return Err(TransportError::UnsupportedAdvertisement(format!(
            "peer speaks protocol v{proto_version}, this build speaks v{TRANSPORT_PROTOCOL_VERSION}"
        )));
    }

    let pk_str = txt(info, "pk")?;
    let pk_bytes = URL_SAFE_NO_PAD.decode(pk_str.as_bytes()).map_err(|e| {
        TransportError::UnsupportedAdvertisement(format!("invalid public key base64: {e}"))
    })?;
    let pk_arr: [u8; 32] = pk_bytes.as_slice().try_into().map_err(|_| {
        TransportError::UnsupportedAdvertisement(
            "public key is not 32 bytes after base64 decode".into(),
        )
    })?;
    let verifying = VerifyingKey::from_bytes(&pk_arr).map_err(|e| {
        TransportError::UnsupportedAdvertisement(format!("public key is not a valid point: {e}"))
    })?;

    let display_name = txt(info, "nm")?.to_string();
    let identity = PeerIdentity::new(&verifying, display_name);
    let peer_id = identity.peer_id.clone();
    let fingerprint = PeerFingerprint::from_verifying_key(&verifying);

    let cf_str = txt(info, "cf")?;
    let cert_fingerprint = cert_sha256_from_b64(cf_str).ok_or_else(|| {
        TransportError::UnsupportedAdvertisement(format!("invalid cert fingerprint '{cf_str}'"))
    })?;

    let pj_str = txt(info, "pj")?;
    let project_id = Uuid::parse_str(pj_str).map_err(|e| {
        TransportError::UnsupportedAdvertisement(format!("invalid project uuid: {e}"))
    })?;

    // Prefer IPv4 if available — it's more common and easier to
    // reason about for loopback / NAT cases. Fall back to any
    // address mDNS resolved for the host.
    let addr = info
        .get_addresses_v4()
        .iter()
        .next()
        .map(|v4| IpAddr::V4(**v4))
        .or_else(|| info.get_addresses().iter().next().copied())
        .ok_or_else(|| {
            TransportError::UnsupportedAdvertisement(format!(
                "no IP addresses for {}",
                info.get_fullname()
            ))
        })?;
    let socket_addr = SocketAddr::new(addr, info.get_port());

    Ok(DiscoveredPeer {
        identity,
        peer_id,
        fingerprint,
        project_id,
        socket_addr,
        cert_fingerprint,
        proto_version,
    })
}

/// Encode the local peer id as a DNS-safe instance name. `PeerId`'s
/// stringification is already URL-safe base64 (no padding) — see
/// [`kcreate_collab::peer::PeerId`] — so this is a defence-in-depth
/// guard rather than a behaviour change.
fn encode_instance_name(peer_id: &PeerId) -> String {
    let raw = peer_id.as_str();
    raw.replace(['/', '+'], "-").replace('=', "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use kcreate_collab::peer::PeerKey;

    #[test]
    fn protocol_version_matches_kcreate_collab() {
        assert_eq!(TRANSPORT_PROTOCOL_VERSION, kcreate_collab::PROTOCOL_VERSION);
    }

    #[test]
    fn instance_name_avoids_dns_unsafe_chars() {
        let key = PeerKey::from_seed([42u8; 32]);
        let identity = key.identity("test-peer");
        let encoded = encode_instance_name(&identity.peer_id);
        assert!(!encoded.contains('/'));
        assert!(!encoded.contains('+'));
        assert!(!encoded.contains('='));
        assert!(encoded.len() >= 22);
    }
}

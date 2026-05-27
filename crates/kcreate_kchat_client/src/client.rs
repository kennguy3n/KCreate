//! High-level [`KChatDesktopClient`] — connection lifecycle, typed
//! method calls, subscription routing, reconnection policy.
//!
//! Sits on top of [`crate::transport::Transport`] and exposes the
//! ergonomic Rust surface the bridge calls into. The client owns the
//! connection state behind a `tokio::Mutex` so concurrent N-API
//! callers (e.g. the bridge's roster-sync tick + a user-driven
//! `share_to_conversation` call) can both make progress without
//! racing on the underlying socket.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::UnixStream;
use tokio::sync::{broadcast, Mutex};
use tokio::time::timeout;

use crate::error::ClientError;
use crate::protocol::{
    CommunitiesListParams, CommunitiesListResult, ConversationsListParams, ConversationsListResult,
    EventsSubscribeParams, EventsSubscribeResult, EventsUnsubscribeParams, GetMembersParams,
    GetMembersResult, GetMembershipParams, IdentityGetParams, KChatCommunity, KChatCommunityMember,
    KChatConversation, KChatIdentity, MembershipAttestation, PostMessageParams, PostMessageResult,
};
use crate::transport::Transport;

/// Default connect timeout per the protocol spec (§2.4).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Default reconnection backoff steps.
pub const RECONNECT_BACKOFF: &[Duration] = &[
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
    Duration::from_secs(30),
];

/// Connection state machine.
enum Connection {
    Disconnected,
    Connected {
        transport: Transport,
        socket_path: PathBuf,
    },
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disconnected => f.debug_tuple("Disconnected").finish(),
            Self::Connected { socket_path, .. } => f
                .debug_struct("Connected")
                .field("socket_path", socket_path)
                .finish(),
        }
    }
}

/// Per-platform default socket paths. The client tries each in order
/// until one connects, so the same code path works whether
/// uney-chat-desktop is using the XDG runtime directory or the
/// fallback `$HOME/.kchat/` location.
#[must_use]
pub fn default_socket_paths() -> Vec<PathBuf> {
    if cfg!(windows) {
        vec![PathBuf::from(r"\\.\pipe\kchat-kcreate")]
    } else {
        let mut paths = Vec::new();
        if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
            paths.push(PathBuf::from(runtime).join("kchat").join("kcreate.sock"));
        }
        if let Some(home) = home_dir() {
            paths.push(home.join(".kchat").join("kcreate.sock"));
        }
        paths
    }
}

fn home_dir() -> Option<PathBuf> {
    // `std::env::home_dir` is deprecated for Windows quirks but we
    // only call it on Unix; reading `$HOME` directly avoids the
    // deprecation lint.
    std::env::var_os("HOME").map(PathBuf::from)
}

/// High-level KChat Desktop IPC client. Owns the connection state
/// and exposes typed methods for each JSON-RPC entry point.
pub struct KChatDesktopClient {
    state: Mutex<Connection>,
    /// Notification re-broadcaster: every successful connect plumbs
    /// the transport's broadcast receiver into a forwarding task
    /// that re-emits onto this sender so subscribers stay connected
    /// across reconnects.
    notification_tx: broadcast::Sender<crate::protocol::CommunityEvent>,
}

impl std::fmt::Debug for KChatDesktopClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KChatDesktopClient").finish()
    }
}

impl Default for KChatDesktopClient {
    fn default() -> Self {
        Self::new()
    }
}

impl KChatDesktopClient {
    /// Build a fresh disconnected client.
    #[must_use]
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            state: Mutex::new(Connection::Disconnected),
            notification_tx: tx,
        }
    }

    /// Subscribe to community notifications. Subscribers stay
    /// attached across reconnects.
    pub fn subscribe_notifications(&self) -> broadcast::Receiver<crate::protocol::CommunityEvent> {
        self.notification_tx.subscribe()
    }

    /// Currently-known connection state.
    pub async fn is_connected(&self) -> bool {
        matches!(*self.state.lock().await, Connection::Connected { .. })
    }

    /// Currently-connected socket path, if any.
    pub async fn connected_path(&self) -> Option<PathBuf> {
        match &*self.state.lock().await {
            Connection::Connected { socket_path, .. } => Some(socket_path.clone()),
            Connection::Disconnected => None,
        }
    }

    /// Attempt to connect to one of the default socket paths. The
    /// first reachable path wins.
    pub async fn connect(&self) -> Result<PathBuf, ClientError> {
        let paths = default_socket_paths();
        if paths.is_empty() {
            return Err(ClientError::Connect {
                path: "<no default socket paths>".into(),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no default KChat Desktop socket paths configured",
                ),
            });
        }
        let mut last_err: Option<ClientError> = None;
        for path in &paths {
            match self.connect_to(path).await {
                Ok(p) => return Ok(p),
                Err(e) => {
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.expect("loop runs at least once when paths is non-empty"))
    }

    /// Attempt to connect to a specific socket path. Replaces any
    /// existing connection.
    pub async fn connect_to(&self, path: &Path) -> Result<PathBuf, ClientError> {
        let transport = open_transport(path)
            .await
            .map_err(|source| ClientError::Connect {
                path: path.display().to_string(),
                source,
            })?;
        self.install_transport(transport, path.to_path_buf()).await;
        Ok(path.to_path_buf())
    }

    async fn install_transport(&self, transport: Transport, socket_path: PathBuf) {
        // Subscribe before swapping state so we never miss a
        // notification arriving between transport-spawn and
        // re-broadcast install.
        let mut sub = transport.subscribe_notifications();
        let notification_tx = self.notification_tx.clone();
        tokio::spawn(async move {
            loop {
                match sub.recv().await {
                    Ok(event) => {
                        let _ = notification_tx.send(event);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        let mut guard = self.state.lock().await;
        // Drop any previously-installed transport so its pump tasks
        // exit cleanly.
        if let Connection::Connected { transport, .. } =
            std::mem::replace(&mut *guard, Connection::Disconnected)
        {
            transport.begin_shutdown();
        }
        *guard = Connection::Connected {
            transport,
            socket_path,
        };
    }

    /// Disconnect any open transport. Idempotent.
    pub async fn disconnect(&self) {
        let prev = {
            let mut guard = self.state.lock().await;
            std::mem::replace(&mut *guard, Connection::Disconnected)
        };
        if let Connection::Connected { transport, .. } = prev {
            transport.await_shutdown().await;
        }
    }

    /// `kchat.identity.get`
    pub async fn get_identity(&self) -> Result<KChatIdentity, ClientError> {
        self.call_typed("kchat.identity.get", Option::<&IdentityGetParams>::None)
            .await
    }

    /// `kchat.communities.list`
    pub async fn list_communities(&self) -> Result<Vec<KChatCommunity>, ClientError> {
        let result: CommunitiesListResult = self
            .call_typed(
                "kchat.communities.list",
                Option::<&CommunitiesListParams>::None,
            )
            .await?;
        Ok(result.communities)
    }

    /// `kchat.communities.getMembers`
    pub async fn get_members(
        &self,
        community_id: &str,
    ) -> Result<Vec<KChatCommunityMember>, ClientError> {
        let params = GetMembersParams {
            community_id: community_id.to_string(),
        };
        let result: GetMembersResult = self
            .call_typed("kchat.communities.getMembers", Some(&params))
            .await?;
        Ok(result.members)
    }

    /// `kchat.communities.getMembership`
    pub async fn get_membership(
        &self,
        community_id: &str,
    ) -> Result<MembershipAttestation, ClientError> {
        let params = GetMembershipParams {
            community_id: community_id.to_string(),
        };
        self.call_typed("kchat.communities.getMembership", Some(&params))
            .await
    }

    /// `kchat.conversations.list`
    pub async fn list_conversations(
        &self,
        community_id: &str,
    ) -> Result<Vec<KChatConversation>, ClientError> {
        let params = ConversationsListParams {
            community_id: community_id.to_string(),
        };
        let result: ConversationsListResult = self
            .call_typed("kchat.conversations.list", Some(&params))
            .await?;
        Ok(result.conversations)
    }

    /// `kchat.conversations.postMessage`
    pub async fn post_message(
        &self,
        params: PostMessageParams,
    ) -> Result<PostMessageResult, ClientError> {
        self.call_typed("kchat.conversations.postMessage", Some(&params))
            .await
    }

    /// `kchat.events.subscribe`
    pub async fn subscribe_community(
        &self,
        community_id: &str,
    ) -> Result<EventsSubscribeResult, ClientError> {
        let params = EventsSubscribeParams {
            community_id: community_id.to_string(),
        };
        self.call_typed("kchat.events.subscribe", Some(&params))
            .await
    }

    /// `kchat.events.unsubscribe`
    pub async fn unsubscribe_community(
        &self,
        subscription_id: &str,
    ) -> Result<serde_json::Value, ClientError> {
        let params = EventsUnsubscribeParams {
            subscription_id: subscription_id.to_string(),
        };
        self.call_typed("kchat.events.unsubscribe", Some(&params))
            .await
    }

    async fn call_typed<P, R>(&self, method: &str, params: Option<&P>) -> Result<R, ClientError>
    where
        P: serde::Serialize + Sync,
        R: serde::de::DeserializeOwned,
    {
        let guard = self.state.lock().await;
        let transport = match &*guard {
            Connection::Connected { transport, .. } => transport,
            Connection::Disconnected => return Err(ClientError::NotConnected),
        };
        transport.call_method(method, params).await
    }
}

async fn open_transport(path: &Path) -> Result<Transport, std::io::Error> {
    #[cfg(unix)]
    {
        let stream = timeout(CONNECT_TIMEOUT, UnixStream::connect(path))
            .await
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "connect timed out")
            })??;
        Ok(Transport::spawn(stream, mint_id_prefix()))
    }
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ClientOptions;
        // Named pipes can be busy briefly during server startup;
        // retry with a short backoff up to the connect-timeout
        // budget. `ERROR_PIPE_BUSY` is the only error we recover
        // from — every other error is fatal.
        const ERROR_PIPE_BUSY: i32 = 231; // winerror.h
        let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
        loop {
            match ClientOptions::new().open(path) {
                Ok(client) => {
                    return Ok(Transport::spawn(client, mint_id_prefix()));
                }
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "named pipe busy and connect timed out",
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

fn mint_id_prefix() -> String {
    use uuid::Uuid;
    let mut s = Uuid::new_v4().simple().to_string();
    s.truncate(8);
    s
}

/// Test convenience: install an arbitrary stream as the
/// connection. Used by the unit-test harness to drive the client
/// against an in-memory duplex pair without needing a real socket.
#[doc(hidden)]
impl KChatDesktopClient {
    pub async fn install_test_stream<S>(&self, stream: S)
    where
        S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        let transport = Transport::spawn(stream, mint_id_prefix());
        self.install_transport(transport, PathBuf::from("<test stream>"))
            .await;
    }
}

//! JSON-RPC 2.0 transport over a local Unix domain socket (Unix) or
//! named pipe (Windows).
//!
//! The transport carries newline-delimited UTF-8 JSON frames. Each
//! frame is either an [`RpcRequest`], an [`RpcResponse`], or a
//! server-initiated [`RpcNotification`]. The transport multiplexes
//! concurrent requests by correlating responses to their request id,
//! routes notifications through a dedicated broadcast channel, and
//! supports graceful shutdown via a `tokio::sync::watch` flag.
//!
//! The transport is generic over the byte stream so the same pump
//! task drives both `tokio::net::UnixStream` and (on Windows) a
//! named-pipe `NamedPipeClient`. The unit test suite uses an
//! in-memory `tokio::io::duplex` pair to exercise the protocol
//! without touching the filesystem.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::error::ClientError;
use crate::protocol::{
    CommunityEvent, ErrorCode, RpcError, RpcNotification, RpcRequest, RpcResponse, JSONRPC_VERSION,
};

/// Default per-request timeout for `call_method`. Long-running
/// server work (e.g. signing a fresh membership attestation) must
/// still complete within this window.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum line length we'll accept on the wire. Matches the spec
/// in `protocol_spec.md` §2.3. A frame larger than this triggers a
/// hard disconnect.
pub const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;

/// Capacity for the inbound notification broadcast channel.
const NOTIFICATION_CHANNEL_CAPACITY: usize = 256;

/// Capacity for the outbound write queue. Backpressure beyond this
/// blocks `call_method` callers, which is the desired behaviour:
/// runaway producers shouldn't be able to grow the queue unbounded.
const OUTBOUND_CHANNEL_CAPACITY: usize = 256;

/// Inbound notification routed to subscribers. We type-narrow to
/// [`CommunityEvent`] because that's the only notification shape
/// uney-chat-desktop is specced to emit.
pub type Notification = CommunityEvent;

/// Pending-request state — `oneshot::Sender` the read task uses to
/// deliver the response back to the `call_method` caller.
type PendingMap = Arc<Mutex<HashMap<String, oneshot::Sender<RpcResponse>>>>;

/// Bi-directional JSON-RPC 2.0 transport for one open connection.
#[derive(Debug)]
pub struct Transport {
    outbound_tx: mpsc::Sender<Vec<u8>>,
    notification_tx: broadcast::Sender<Notification>,
    pending: PendingMap,
    write_handle: JoinHandle<()>,
    read_handle: JoinHandle<()>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// Monotonic counter the client uses to mint request ids. Each
    /// id is prefixed with a per-connection nonce so concurrent
    /// connections never collide (matters for the test harness;
    /// production deployments only ever have one connection
    /// outstanding).
    next_id: Arc<std::sync::atomic::AtomicU64>,
    id_prefix: String,
}

impl Transport {
    /// Spawn the read + write pump tasks on the supplied stream.
    /// The returned transport multiplexes concurrent
    /// `call_method` invocations and broadcasts inbound
    /// notifications to subscribers.
    pub fn spawn<S>(stream: S, id_prefix: impl Into<String>) -> Self
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (read_half, write_half) = tokio::io::split(stream);
        let (outbound_tx, outbound_rx) = mpsc::channel::<Vec<u8>>(OUTBOUND_CHANNEL_CAPACITY);
        let (notification_tx, _) =
            broadcast::channel::<Notification>(NOTIFICATION_CHANNEL_CAPACITY);
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        let write_handle = tokio::spawn(write_pump(write_half, outbound_rx, shutdown_rx.clone()));
        let read_handle = tokio::spawn(read_pump(
            read_half,
            pending.clone(),
            notification_tx.clone(),
            shutdown_rx,
        ));

        Self {
            outbound_tx,
            notification_tx,
            pending,
            write_handle,
            read_handle,
            shutdown_tx,
            next_id: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            id_prefix: id_prefix.into(),
        }
    }

    /// Send a JSON-RPC 2.0 request and await the matching response.
    /// Returns the typed `Result` payload on success or the typed
    /// error code from the server on failure.
    pub async fn call_method<P, R>(
        &self,
        method: &str,
        params: Option<&P>,
    ) -> Result<R, ClientError>
    where
        P: Serialize + Sync,
        R: DeserializeOwned,
    {
        let id = self.allocate_id();
        let params_value = match params {
            Some(p) => Some(serde_json::to_value(p).map_err(ClientError::Serialization)?),
            None => None,
        };
        let req = RpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: method.to_string(),
            params: params_value,
            id: id.clone(),
        };
        let (tx, rx) = oneshot::channel::<RpcResponse>();
        self.pending.lock().await.insert(id.clone(), tx);

        let line = serialize_frame(&req)?;
        if self.outbound_tx.send(line).await.is_err() {
            // Outbound pump terminated — clean up the pending entry.
            self.pending.lock().await.remove(&id);
            return Err(ClientError::Disconnected);
        }

        let resp = match timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(_)) => {
                self.pending.lock().await.remove(&id);
                return Err(ClientError::Disconnected);
            }
            Err(_) => {
                self.pending.lock().await.remove(&id);
                return Err(ClientError::Timeout(method.to_string()));
            }
        };

        match (resp.result, resp.error) {
            (Some(value), None) => {
                serde_json::from_value::<R>(value).map_err(ClientError::Deserialization)
            }
            (None, Some(err)) => Err(ClientError::Rpc(err)),
            (Some(_), Some(_)) => Err(ClientError::Protocol(
                "response carried both result and error".into(),
            )),
            (None, None) => Err(ClientError::Protocol(
                "response carried neither result nor error".into(),
            )),
        }
    }

    /// Subscribe to inbound notifications. Each subscriber receives
    /// every notification emitted after `subscribe` is called.
    pub fn subscribe_notifications(&self) -> broadcast::Receiver<Notification> {
        self.notification_tx.subscribe()
    }

    /// Initiate graceful shutdown. Pumps observe the flag and exit
    /// at the next safe boundary. `await_shutdown` blocks until
    /// both pumps have terminated.
    pub fn begin_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Wait for the read and write pump tasks to terminate. Calls
    /// `begin_shutdown` first.
    pub async fn await_shutdown(self) {
        self.begin_shutdown();
        // Drop the outbound sender so the write pump's `recv` returns
        // `None` and the task exits cleanly.
        drop(self.outbound_tx);
        let _ = self.write_handle.await;
        let _ = self.read_handle.await;
    }

    fn allocate_id(&self) -> String {
        let n = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("{}-{n}", self.id_prefix)
    }
}

fn serialize_frame<T: Serialize>(value: &T) -> Result<Vec<u8>, ClientError> {
    let mut buf = serde_json::to_vec(value).map_err(ClientError::Serialization)?;
    if buf.len() + 1 > MAX_FRAME_BYTES {
        return Err(ClientError::Protocol(format!(
            "outbound frame exceeds {MAX_FRAME_BYTES}-byte limit (got {} bytes)",
            buf.len()
        )));
    }
    buf.push(b'\n');
    Ok(buf)
}

async fn write_pump<W>(
    mut writer: W,
    mut rx: mpsc::Receiver<Vec<u8>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    W: AsyncWrite + Unpin + Send + 'static,
{
    loop {
        tokio::select! {
            // Shutdown observed — drain remaining outbound frames
            // then exit. We bias the select to drain first so an
            // in-flight `call_method` completes cleanly.
            res = rx.recv() => {
                let Some(frame) = res else { break };
                if let Err(e) = writer.write_all(&frame).await {
                    tracing::warn!(error = %e, "kchat-client: write failed, shutting down");
                    break;
                }
                if let Err(e) = writer.flush().await {
                    tracing::warn!(error = %e, "kchat-client: flush failed, shutting down");
                    break;
                }
            }
            res = shutdown.changed() => {
                if res.is_err() || *shutdown.borrow() {
                    break;
                }
            }
        }
    }
    let _ = writer.shutdown().await;
}

async fn read_pump<R>(
    reader: R,
    pending: PendingMap,
    notification_tx: broadcast::Sender<Notification>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut buf = BufReader::with_capacity(64 * 1024, reader);
    let mut line = Vec::with_capacity(2048);
    loop {
        line.clear();
        let n = tokio::select! {
            res = buf.read_until(b'\n', &mut line) => res,
            res = shutdown.changed() => {
                if res.is_err() || *shutdown.borrow() {
                    break;
                }
                continue;
            }
        };
        match n {
            Ok(0) => break, // EOF
            Ok(read) => {
                if read > MAX_FRAME_BYTES {
                    tracing::warn!(read, "kchat-client: oversize frame, disconnecting");
                    break;
                }
                if line.last() == Some(&b'\n') {
                    line.pop();
                }
                if line.is_empty() {
                    continue;
                }
                if let Err(e) = dispatch_frame(&line, &pending, &notification_tx).await {
                    tracing::warn!(error = %e, "kchat-client: frame dispatch failed");
                    // Don't disconnect on a single bad frame; spec
                    // requires tolerating unknown fields. But on a
                    // hard parse error there's nothing to do.
                    if matches!(e, ClientError::Protocol(_)) {
                        break;
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "kchat-client: read failed");
                break;
            }
        }
    }
    // Drain pending callers so their `oneshot` rxs report
    // `Disconnected`.
    let mut pending = pending.lock().await;
    pending.clear();
}

async fn dispatch_frame(
    raw: &[u8],
    pending: &PendingMap,
    notification_tx: &broadcast::Sender<Notification>,
) -> Result<(), ClientError> {
    let value: serde_json::Value =
        serde_json::from_slice(raw).map_err(ClientError::Deserialization)?;

    // Notifications carry a `method` field and no `id`. Responses
    // carry an `id` and either `result` or `error`. We disambiguate
    // by inspecting the `id` and `method` keys.
    let has_id = value.get("id").is_some();
    let has_method = value.get("method").is_some();

    if !has_id && has_method {
        let notif: RpcNotification =
            serde_json::from_value(value).map_err(ClientError::Deserialization)?;
        if notif.method == "kchat.events.notify" {
            match serde_json::from_value::<Notification>(notif.params) {
                Ok(event) => {
                    let _ = notification_tx.send(event);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "kchat-client: skipping unparseable notify payload");
                }
            }
        } else {
            tracing::debug!(method = %notif.method, "kchat-client: ignoring unknown notification");
        }
        return Ok(());
    }

    let resp: RpcResponse = serde_json::from_value(value).map_err(ClientError::Deserialization)?;
    let mut pending = pending.lock().await;
    if let Some(tx) = pending.remove(&resp.id) {
        let _ = tx.send(resp);
    } else {
        tracing::debug!(id = %resp.id, "kchat-client: response with unknown id, dropping");
    }
    Ok(())
}

/// Helper used by the mock server to build an error response.
#[must_use]
pub fn make_error_response(id: String, code: ErrorCode, message: impl Into<String>) -> RpcResponse {
    RpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id,
        result: None,
        error: Some(RpcError {
            code: code.as_i32(),
            message: message.into(),
            data: None,
        }),
    }
}

/// Helper used by the mock server to build a success response.
pub fn make_ok_response<T: Serialize>(id: String, result: &T) -> Result<RpcResponse, ClientError> {
    Ok(RpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id,
        result: Some(serde_json::to_value(result).map_err(ClientError::Serialization)?),
        error: None,
    })
}

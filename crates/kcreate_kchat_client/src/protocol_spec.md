# KChat Desktop Local IPC Protocol — KCreate ↔ uney-chat-desktop

**Version:** 1
**Status:** KCreate defines this protocol. The server side runs inside
`uneycom/uney-chat-desktop` and is not yet implemented at the time of
this writing; this document is the contract.
**Owner:** KCreate (`kennguy3n/KCreate`,
`crates/kcreate_kchat_client/src/protocol_spec.md`).
**Audience:** uney-chat-desktop maintainers implementing the server
side.

## 1. Scope

KCreate ships a real-time multi-user collaboration system gated on
KChat community membership. The collaboration crate
(`kcreate_collab`) accepts only peers presenting a valid Ed25519-
signed `KChatMembership` attestation. Previously the only way to
obtain that attestation was the in-tree dev issuer
(`kcreate_kchat::DevIssuer`, behind the `kchat-dev-issuer` feature
flag). Phase 7 introduces a **production path**: KCreate connects
locally to the user's running uney-chat-desktop instance over a Unix
domain socket (macOS / Linux) or Windows named pipe and asks for a
fresh attestation signed by the community's identity key.

The protocol carries:

* User identity (JID, display name, Ed25519 public key, derived
  peer id).
* The set of communities the user belongs to, with role.
* Community member rosters, with role.
* Signed membership attestations on demand.
* Conversation (channel + DM) listings for a community.
* Outbound messages to a conversation, including a custom
  rich-card payload for KCreate document share invites.
* A streaming subscription channel for member join / leave /
  role-change / presence updates inside a community.

Out of scope:

* Direct chat messaging UI (KCreate does not render the KChat
  message list — it only posts custom-content invites that
  uney-chat-desktop renders as rich cards).
* MLS group internals (KCreate consumes the derived Ed25519
  identity key only — never the MLS group secrets themselves).
* Server-to-server federation (this is a local-process IPC).

## 2. Transport

### 2.1 Socket path (Unix: macOS, Linux)

```
$XDG_RUNTIME_DIR/kchat/kcreate.sock    (when XDG_RUNTIME_DIR is set)
$HOME/.kchat/kcreate.sock              (fallback)
```

The directory MUST be created with mode `0700`. The socket file
MUST be created with mode `0600` (only the owning user can connect).
Both filesystem locations are acceptable; the KCreate client tries
`$XDG_RUNTIME_DIR/kchat/kcreate.sock` first and falls back to
`$HOME/.kchat/kcreate.sock`.

### 2.2 Pipe name (Windows)

```
\\.\pipe\kchat-kcreate
```

The pipe is created with a security descriptor that grants access
only to the user that owns the uney-chat-desktop process. KCreate's
client opens it with read+write access.

### 2.3 Framing

The transport carries one **JSON object per line**: a UTF-8 encoded
JSON document followed by a single `\n` byte. Servers MUST NOT
embed unescaped newlines inside the JSON. Each line is a complete
JSON-RPC 2.0 message — a request, a response, or a notification.

Maximum frame length: **2 MiB** (2 × 1024 × 1024 bytes). A frame
larger than this MUST be rejected by both sides with
`InvalidRequest` (`-32600`) and the connection closed.

### 2.4 Timeouts

* **Connect:** the KCreate client gives up if the server does not
  accept the connection within **5 seconds**.
* **Per-request:** the client gives up on an outstanding request
  after **10 seconds** if no response correlating to the request id
  arrives. (Subscription notifications do not count against this —
  they are correlated by `subscription_id` inside the params, not
  by JSON-RPC id.)
* **Heartbeat:** servers MAY emit a no-op `kchat.events.notify`
  with `kind = "ping"` once per 30 s on an active subscription to
  keep NAT / mDNS state warm.

### 2.5 Reconnection

KCreate clients reconnect on socket close using exponential
backoff: `1 s, 2 s, 4 s, 8 s, 16 s, 30 s`, then steady at 30 s.
On reconnect the client re-subscribes to whatever community it
was subscribed to previously and re-requests the membership
attestation (the previous one may have expired).

### 2.6 Authentication

uney-chat-desktop is the trust root: only processes with
filesystem permission to open the socket may connect, and on
Unix the socket permission bits (`0600`) restrict that to the
same user account. There is no extra application-layer
authentication — the IPC handshake is the user opening their
own running KChat Desktop.

If uney-chat-desktop wants stricter pairing (e.g. to gate
which local apps can request memberships), it MAY require the
client to call `kchat.identity.get` first; in that case the
server records the peer id (BLAKE3 of the local user's Ed25519
key) in its audit log so a future "see which apps used your
KChat identity" panel can surface the access.

## 3. JSON-RPC 2.0 envelope

### 3.1 Request

```json
{ "jsonrpc": "2.0", "method": "kchat.identity.get", "id": "rq-1" }
{ "jsonrpc": "2.0", "method": "kchat.communities.list", "id": "rq-2" }
```

* `jsonrpc` MUST be `"2.0"`.
* `id` MUST be a non-empty string. KCreate uses UUIDs.
* `params` is optional; absent or `null` for parameterless methods.

### 3.2 Response (success)

```json
{
  "jsonrpc": "2.0",
  "id": "rq-1",
  "result": { "jid": "ken@kchat.com", "displayName": "Ken", "publicKey": "...", "peerId": "..." }
}
```

### 3.3 Response (error)

```json
{
  "jsonrpc": "2.0",
  "id": "rq-1",
  "error": { "code": -32003, "message": "community not found" }
}
```

### 3.4 Notification (server → client only)

```json
{
  "jsonrpc": "2.0",
  "method": "kchat.events.notify",
  "params": {
    "subscriptionId": "sub-1",
    "communityId": "comm-1",
    "at": "2026-05-27T12:00:00Z",
    "event": { "kind": "memberJoined", "member": {...} }
  }
}
```

Notifications have no `id` and never receive a response.

## 4. Methods

All method names use dotted lowercase (`category.action`). Casing
of object fields is `camelCase` on the wire.

### 4.1 `kchat.identity.get`

Returns the local user's KChat identity.

* **Params:** none (`null` or omitted).
* **Result:**
  ```json
  {
    "jid": "ken@kchat.com",
    "displayName": "Ken",
    "publicKey": "<base64url-no-pad 32-byte Ed25519 verifying key>",
    "peerId": "<BLAKE3 22-char prefix of publicKey>"
  }
  ```
* **Errors:** `NotAuthenticated (-32001)` if no user is signed in.

### 4.2 `kchat.communities.list`

* **Params:** none.
* **Result:**
  ```json
  {
    "communities": [
      {
        "id": "comm-uneydev",
        "name": "Uney Devs",
        "description": "Engineering chat",
        "memberCount": 24,
        "role": "admin"
      }
    ]
  }
  ```
* **Errors:** `NotAuthenticated` if no user is signed in.

### 4.3 `kchat.communities.getMembers`

* **Params:** `{ "communityId": "<id>" }`
* **Result:**
  ```json
  {
    "members": [
      { "jid": "ken@kchat.com", "displayName": "Ken",
        "publicKey": "...", "peerId": "...", "role": "owner" }
    ]
  }
  ```
* **Errors:** `NotFound (-32003)` if the community is not visible
  to the local user.

### 4.4 `kchat.communities.getMembership`

Returns a signed membership attestation for the local user. The
attestation is Ed25519-signed by the community's identity key
(derived from the community's MLS identity). KCreate verifies the
signature against the same `issuerPublicKey` it received in
`role: owner|admin|member` decisions; the attestation is bound
to the local user's `publicKey` and `peerId` so it cannot be
lifted onto another peer.

* **Params:** `{ "communityId": "<id>" }`
* **Result:**
  ```json
  {
    "issuerPublicKey": "<base64url-no-pad community signing key>",
    "groupId": "comm-uneydev",
    "peerId": "<local user peer id>",
    "peerPublicKey": "<local user Ed25519 pubkey, base64url-no-pad>",
    "issuedAt": "2026-05-27T12:00:00Z",
    "expiresAt": "2026-05-27T13:00:00Z",
    "signature": "<base64url-no-pad Ed25519 signature>"
  }
  ```

The signed canonical view is computed by serialising the fields
**in declaration order**, namespaced into a JSON object identical to
`kcreate_collab::kchat::KChatMembership` minus the `signature`
field. KCreate's verifier reproduces the exact same byte layout —
adding fields requires bumping the wire-format version.

Recommended issuance lifetime: **1 hour**. KCreate clients trigger a
refresh once the remaining lifetime drops below 5 minutes.

* **Errors:**
  * `NotFound` if the community is unknown.
  * `PermissionDenied (-32002)` if the user is no longer a member
    (revoked since last login). KCreate surfaces this as "you were
    removed from the community" and tears down the active
    collaboration session.

### 4.5 `kchat.conversations.list`

* **Params:** `{ "communityId": "<id>" }`
* **Result:**
  ```json
  {
    "conversations": [
      { "id": "conv-general", "name": "general",
        "communityId": "comm-uneydev", "conversationType": "channel" }
    ]
  }
  ```
* **Errors:** `NotFound` if the community is not visible.

### 4.6 `kchat.conversations.postMessage`

Posts a message to a conversation. The `payload` is opaque to
uney-chat-desktop unless `contentType` is set — in that case the
message is rendered as a rich card via the extension platform's
custom content registry.

* **Params:**
  ```json
  {
    "conversationId": "conv-general",
    "contentType": "kcreate.invite.v1",
    "payload": { "schemaVersion": 1, "projectId": "...", ... }
  }
  ```

The `kcreate.invite.v1` payload shape is:

```json
{
  "schemaVersion": 1,
  "projectId": "<uuid>",
  "projectName": "MyProject",
  "ownerPeerId": "...",
  "ownerPublicKey": "...",
  "ownerDisplayName": "Ken",
  "certFingerprint": "<base64 sha256 of QUIC leaf cert>",
  "ownerSocketAddr": "192.168.1.10:55321",
  "communityId": "comm-uneydev",
  "conversationId": "conv-general",
  "issuedAt": "2026-05-27T12:00:00Z"
}
```

* **Result:**
  ```json
  { "messageId": "<server id>", "postedAt": "2026-05-27T12:00:01Z" }
  ```

### 4.7 `kchat.events.subscribe` / `kchat.events.unsubscribe`

Open / close a streaming notification channel for a community.

* **Subscribe params:** `{ "communityId": "<id>" }`
* **Subscribe result:** `{ "subscriptionId": "<server id>" }`
* **Unsubscribe params:** `{ "subscriptionId": "<id>" }`
* **Unsubscribe result:** `{}`

While a subscription is active, the server pushes
`kchat.events.notify` notifications. The KCreate client routes
each event to the bridge which fans it out to the renderer.

Event kinds (`event.kind` in the notification):

* `memberJoined` — payload `{ member: <KChatCommunityMember> }`.
* `memberLeft` — payload `{ peerId, jid }`.
* `memberRoleChanged` — payload `{ peerId, jid, newRole }`.
* `memberPresence` — payload `{ peerId, jid, online: true|false }`.
* `ping` — keep-alive, no payload.

## 5. Error codes

| Code   | Meaning                                                 |
| ------ | ------------------------------------------------------- |
| -32700 | Parse error (invalid JSON)                              |
| -32600 | Invalid request (not a JSON-RPC 2.0 envelope)           |
| -32601 | Method not found                                        |
| -32602 | Invalid params                                          |
| -32603 | Internal server error                                   |
| -32001 | Not authenticated (no active KChat user)                |
| -32002 | Permission denied                                       |
| -32003 | Resource not found                                      |
| -32004 | Subscription already active for this community          |
| -32005 | Server is shutting down                                 |

The KChat-specific codes (-32001 .. -32005) live in the JSON-RPC
implementation-defined range. Server implementations MUST NOT use
codes outside this range or the standard -32600 .. -32700 range.

## 6. Versioning

* This document is **version 1** of the protocol.
* Additive changes (new methods, new event kinds, new optional
  fields) do not require a version bump. Both sides MUST tolerate
  unknown optional fields and unknown event kinds without crashing
  (skip-and-log).
* Incompatible changes (removed fields, changed semantics, changed
  signing layout) require a new protocol version. The transport
  carries the version in the connection-level header documented
  separately (when introduced); until then the version is implicit
  and locked at 1.
* Concrete schema versions for embedded payloads (e.g.
  `InviteCardPayload::schemaVersion`) are tracked independently.

## 7. Security model

* The socket / pipe is restricted to the local user via OS
  permissions. No network listener is opened — KCreate must run
  on the same machine as uney-chat-desktop.
* uney-chat-desktop is the trust root. The Ed25519 community
  signing key never leaves the KChat process; KCreate only ever
  sees the public half (`issuerPublicKey`) and the resulting
  signed attestations.
* Membership attestations are short-lived (recommended 1 h). They
  bind the local user's Ed25519 public key, so they cannot be
  lifted onto another peer.
* KCreate never persists a membership across restarts. On each
  app launch it re-asks uney-chat-desktop for a fresh
  attestation; if uney-chat-desktop isn't running, multiplayer
  remains locked.
* Audit trail: when implemented on the uney-chat-desktop side,
  every `getMembership` call should be logged so the user can
  audit which local apps requested attestations and when.

## 8. Reference implementation

The mock server used by KCreate's test suite (see
`crates/kcreate_kchat_client/src/tests/mock_server.rs`) is the
canonical Rust reference. It implements every method and emits
the same notification shape uney-chat-desktop is expected to.
uney-chat-desktop maintainers can use it as a behavioural reference
and as a contract test fixture during their implementation.

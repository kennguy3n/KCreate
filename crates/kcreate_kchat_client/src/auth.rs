//! In-memory token store + login/refresh policy for the KChat
//! backend REST client.
//!
//! KCreate **never persists tokens to disk**. The store lives
//! inside the [`crate::client::KChatBackendClient`] for the
//! lifetime of the bridge — if the process exits, the user must
//! sign in again. This matches KChat Desktop's own session model.
//!
//! ## Refresh policy
//!
//! - On every authenticated request, we attach the cached access
//!   token via `Authorization: Bearer <token>`.
//! - If the response is `401 Unauthorized` AND we have a refresh
//!   token, we transparently call
//!   `POST /api/v1/auth/refresh`, replace the cached tokens, and
//!   retry the original request **once**. If that retry also
//!   returns 401, we surface
//!   [`ClientError::RefreshExpired`](crate::error::ClientError::RefreshExpired)
//!   so the renderer can prompt for a fresh login.
//! - We also pre-emptively refresh when the access token is within
//!   30 seconds of its `expires_at` — this avoids burning a round
//!   trip on the 401-then-retry path during burst traffic.

use std::time::Duration;

use chrono::{DateTime, Utc};
use parking_lot::RwLock;

use crate::protocol::{KChatIdentity, LoginResponse, RefreshResponse};

/// Pre-emptive refresh window. The client refreshes the access
/// token when it has less than this much lifetime remaining.
pub const PREEMPTIVE_REFRESH_WINDOW: Duration = Duration::from_secs(30);

/// A snapshot of the credentials KCreate is authenticated with.
/// Stored only in memory; the renderer is responsible for asking
/// the user to re-enter their password on next launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenSet {
    /// Bearer access token. Sent verbatim in
    /// `Authorization: Bearer <token>` on every authenticated
    /// request.
    pub access_token: String,
    /// Long-lived refresh token. Used to mint a new access token
    /// when the cached one expires. Never sent over plain HTTP.
    pub refresh_token: String,
    /// Wall-clock expiry of the access token.
    pub expires_at: DateTime<Utc>,
    /// Identity of the authenticated user, captured at login.
    /// The bridge re-exports this through `kchat_backend_status`.
    pub identity: KChatIdentity,
}

impl TokenSet {
    /// Build a token set from a fresh `POST /api/v1/auth/login`
    /// response.
    #[must_use]
    pub fn from_login(resp: LoginResponse, now: DateTime<Utc>) -> Self {
        let expires_at =
            now + chrono::Duration::seconds(resp.expires_in_seconds.min(i64::MAX as u64) as i64);
        Self {
            access_token: resp.access_token,
            refresh_token: resp.refresh_token,
            expires_at,
            identity: resp.identity,
        }
    }

    /// Apply a successful `POST /api/v1/auth/refresh` response in
    /// place — preserves the cached identity (the refresh endpoint
    /// doesn't return identity, only fresh tokens).
    pub fn apply_refresh(&mut self, resp: RefreshResponse, now: DateTime<Utc>) {
        self.access_token = resp.access_token;
        self.refresh_token = resp.refresh_token;
        self.expires_at =
            now + chrono::Duration::seconds(resp.expires_in_seconds.min(i64::MAX as u64) as i64);
    }

    /// True if the access token is past or near its expiry. The
    /// REST wrapper uses this to pre-emptively refresh before
    /// burning a request on the 401-then-retry path.
    #[must_use]
    pub fn needs_preemptive_refresh(&self, now: DateTime<Utc>) -> bool {
        let remaining = self.expires_at.signed_duration_since(now);
        let window = chrono::Duration::from_std(PREEMPTIVE_REFRESH_WINDOW)
            .unwrap_or_else(|_| chrono::Duration::seconds(0));
        remaining <= window
    }
}

/// Thread-safe in-memory token store. Cheap to clone (wraps an
/// `Arc<RwLock<…>>` via the `parking_lot::RwLock` field).
///
/// We expose explicit `snapshot` / `replace` rather than a "give
/// me the access token" accessor to keep the lock-acquisition
/// discipline in one place — the rest client always reads + retries
/// against the same snapshot rather than two separately-locked
/// reads.
#[derive(Debug, Default)]
pub struct TokenStore {
    inner: RwLock<Option<TokenSet>>,
}

impl TokenStore {
    /// Build an empty store. The client starts out unauthenticated.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            inner: RwLock::new(None),
        }
    }

    /// Snapshot the current token set, if any.
    pub fn snapshot(&self) -> Option<TokenSet> {
        self.inner.read().clone()
    }

    /// Overwrite the current token set wholesale (used after
    /// login).
    pub fn replace(&self, tokens: TokenSet) {
        *self.inner.write() = Some(tokens);
    }

    /// Clear the token set (used on logout or after a refresh
    /// failure).
    pub fn clear(&self) {
        *self.inner.write() = None;
    }

    /// Apply a refresh response in place. Returns the updated
    /// snapshot, or `None` if nobody is logged in (the caller
    /// should treat that as "refresh raced a logout").
    pub fn apply_refresh(&self, resp: RefreshResponse, now: DateTime<Utc>) -> Option<TokenSet> {
        let mut guard = self.inner.write();
        let tokens = guard.as_mut()?;
        tokens.apply_refresh(resp, now);
        Some(tokens.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;

    fn identity() -> KChatIdentity {
        KChatIdentity {
            jid: "alice@kchat.com".into(),
            display_name: "Alice".into(),
            public_key: "AAA".into(),
            peer_id: "peer-alice".into(),
        }
    }

    #[test]
    fn from_login_computes_expires_at() {
        let now = Utc::now();
        let resp = LoginResponse {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_in_seconds: 3600,
            identity: identity(),
        };
        let tokens = TokenSet::from_login(resp, now);
        let diff = tokens.expires_at.signed_duration_since(now);
        assert!(diff >= ChronoDuration::seconds(3599));
        assert!(diff <= ChronoDuration::seconds(3601));
    }

    #[test]
    fn needs_preemptive_refresh_when_inside_window() {
        let now = Utc::now();
        let tokens = TokenSet {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: now + ChronoDuration::seconds(10),
            identity: identity(),
        };
        assert!(tokens.needs_preemptive_refresh(now));
    }

    #[test]
    fn does_not_preemptively_refresh_when_outside_window() {
        let now = Utc::now();
        let tokens = TokenSet {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: now + ChronoDuration::seconds(120),
            identity: identity(),
        };
        assert!(!tokens.needs_preemptive_refresh(now));
    }

    #[test]
    fn store_replace_then_snapshot_round_trips() {
        let store = TokenStore::new();
        assert!(store.snapshot().is_none());
        let now = Utc::now();
        let tokens = TokenSet {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: now + ChronoDuration::seconds(60),
            identity: identity(),
        };
        store.replace(tokens.clone());
        assert_eq!(store.snapshot().unwrap(), tokens);
        store.clear();
        assert!(store.snapshot().is_none());
    }

    #[test]
    fn store_apply_refresh_updates_in_place() {
        let store = TokenStore::new();
        let now = Utc::now();
        let initial = TokenSet {
            access_token: "old".into(),
            refresh_token: "old-r".into(),
            expires_at: now + ChronoDuration::seconds(10),
            identity: identity(),
        };
        store.replace(initial);
        let updated = store
            .apply_refresh(
                RefreshResponse {
                    access_token: "new".into(),
                    refresh_token: "new-r".into(),
                    expires_in_seconds: 3600,
                },
                now,
            )
            .expect("refresh updates");
        assert_eq!(updated.access_token, "new");
        assert_eq!(updated.refresh_token, "new-r");
        assert_eq!(updated.identity.display_name, "Alice");
    }

    #[test]
    fn apply_refresh_no_op_when_logged_out() {
        let store = TokenStore::new();
        let res = store.apply_refresh(
            RefreshResponse {
                access_token: "n".into(),
                refresh_token: "r".into(),
                expires_in_seconds: 3600,
            },
            Utc::now(),
        );
        assert!(res.is_none());
    }
}

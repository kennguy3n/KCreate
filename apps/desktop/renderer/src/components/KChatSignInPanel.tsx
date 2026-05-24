// KChatSignInPanel — multiplayer gate UI (Phase 4 follow-up).
//
// Renders the sign-in / sign-out affordance for the KChat group
// authority that gates KCreate multiplayer. There are two modes:
//
//   1. **Locked** — the user is not signed into a KChat group.
//      We surface a textarea to paste a JSON `KChatInstallRequest`
//      minted by the real (out-of-tree) KChat client. Bridges
//      built with the `kchat-dev-issuer` feature also offer a
//      "Mint dev membership" affordance backed by the in-process
//      `kcreate_kchat::DevIssuer` so engineers can drive the
//      multiplayer pipeline end-to-end without a live KChat server.
//
//   2. **Signed in** — the gate is open. We show the group id,
//      peer id, and an "Expires in …" countdown, plus a sign-out
//      button that calls `kchat.clear()` and re-locks the gate.
//
// The panel deliberately does NOT manage the peer roster, dial
// flow, or session state — that's `PresencePanel`'s job. We sit
// above it in the layout (or fully replace it when locked) so the
// user sees one UI surface for "am I in a group" and another for
// "am I in a session", and the two don't conflict.

import { useCallback, useEffect, useMemo, useState } from "react";

import type {
  KChatDevMintRequest,
  KChatInstallRequest,
  KChatLocalIdentity,
  KChatMembershipStatus,
  TrustedIssuer,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

/// LocalStorage key shared with `PresencePanel.loadOrGenerateSeed()`.
/// Read-only here — we look up the seed to derive the local peer
/// public key for the membership binding, but never rotate it.
const SEED_STORAGE_KEY = "kcreate.session.seed";

/// Persistent UI affordance state: the most recently used group
/// id, plus the dev validity (in hours). Stored separately from
/// the gate state because the gate state is owned by the bridge,
/// not the renderer.
const LAST_GROUP_ID_STORAGE_KEY = "kcreate.kchat.lastGroupId";

/// Persistent dev issuer seed (for the dev-only mint path). Same
/// seed across sessions → same issuer trust root → re-signing
/// in keeps the peer id stable for any peers that previously
/// trusted it. Stored as base64url, 32 bytes (Ed25519 seed).
const DEV_ISSUER_SEED_STORAGE_KEY = "kcreate.kchat.devIssuerSeed";

export interface KChatSignInPanelProps {
  /// Current gate state. `null` means we haven't loaded it yet
  /// (treat as locked, conservative default).
  status: KChatMembershipStatus | null;
  /// Called by the panel whenever the gate state changes (after
  /// install / clear). The parent reflects this back into
  /// `PresencePanel` so it switches between the locked CTA and
  /// the live multiplayer UI.
  onStatusChange: (next: KChatMembershipStatus) => void;
  /// Status bubble to the editor footer (mirrors PresencePanel).
  onStatus?: (msg: string | null) => void;
}

export function KChatSignInPanel({
  status,
  onStatusChange,
  onStatus,
}: KChatSignInPanelProps): JSX.Element {
  const locked = status === null || status.locked;
  const [devIssuerAvailable, setDevIssuerAvailable] = useState<boolean>(false);
  const [localIdentity, setLocalIdentity] = useState<KChatLocalIdentity | null>(
    null,
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Paste-attestation flow state.
  const [pasteJson, setPasteJson] = useState<string>("");

  // Dev mint flow state.
  const [groupId, setGroupId] = useState<string>(() =>
    loadLastGroupId() ?? "kcreate-dev-group",
  );
  const [devValidityHours, setDevValidityHours] = useState<number>(24);

  // Probe the dev issuer feature flag on mount (only used when
  // locked — once signed in we don't need it). The probe is cheap
  // (a cfg!() check on the Rust side) but we still cache it.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const avail = await window.kcreate.kchat.devIssuerAvailable();
        if (!cancelled) setDevIssuerAvailable(avail);
      } catch {
        if (!cancelled) setDevIssuerAvailable(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Live trusted-issuer allowlist. Loaded once on mount; mutated
  // in place by the add/remove flows below so the panel re-renders
  // without an extra bridge round-trip. An empty list means the
  // bridge accepts any issuer (backward-compat with the dev flow),
  // which we surface explicitly in the management section so the
  // user understands the trust posture.
  const [trustedIssuers, setTrustedIssuers] = useState<TrustedIssuer[]>([]);
  const [trustedIssuersError, setTrustedIssuersError] = useState<string | null>(
    null,
  );
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const list = await window.kcreate.kchat.trustedIssuers();
        if (!cancelled) setTrustedIssuers(list);
      } catch (e) {
        if (!cancelled) setTrustedIssuersError(errMsg(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const handleAddTrustedIssuer = useCallback(
    async (input: { issuerPublicKey: string; label: string }): Promise<void> => {
      setTrustedIssuersError(null);
      try {
        // `addedAt` is canonicalised on the Rust side regardless
        // of what we send, but the schema requires the field so
        // we provide "now".
        const next = await window.kcreate.kchat.addTrustedIssuer({
          issuerPublicKey: input.issuerPublicKey,
          label: input.label,
          addedAt: new Date().toISOString(),
        });
        setTrustedIssuers(next);
      } catch (e) {
        setTrustedIssuersError(errMsg(e));
        // Re-throw so the form component (TrustedIssuersSection)
        // can distinguish success from failure and preserve the
        // user's typed inputs after a validation/bridge rejection.
        // The error itself is already surfaced through
        // `trustedIssuersError` above, so the form's catch branch
        // is bodyless.
        throw e;
      }
    },
    [],
  );
  const handleRemoveTrustedIssuer = useCallback(
    async (issuerPublicKey: string): Promise<void> => {
      setTrustedIssuersError(null);
      try {
        const next =
          await window.kcreate.kchat.removeTrustedIssuer(issuerPublicKey);
        setTrustedIssuers(next);
      } catch (e) {
        setTrustedIssuersError(errMsg(e));
      }
    },
    [],
  );

  // Derive the local peer identity from the persistent seed.
  // Needed for the dev mint path (to bind the attestation to this
  // peer's key) AND for displaying the "your peer id" hint in
  // the locked-state UI so the user knows what public key the
  // KChat client should mint a membership for.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const seed = loadSeedOrNull();
        if (seed === null) {
          if (!cancelled) setLocalIdentity(null);
          return;
        }
        const id = await window.kcreate.kchat.deriveLocalIdentity(seed);
        if (!cancelled) setLocalIdentity(id);
      } catch (e) {
        if (!cancelled) {
          setLocalIdentity(null);
          setError(
            `Could not derive local peer identity: ${errMsg(e)}. ` +
              "The KChat sign-in form needs the local Ed25519 keypair " +
              "to bind a membership to.",
          );
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // ----------------------------------------------------------------
  // Paste-attestation flow.
  // ----------------------------------------------------------------
  const handleInstall = useCallback(async (): Promise<void> => {
    setError(null);
    setBusy(true);
    try {
      const trimmed = pasteJson.trim();
      if (trimmed.length === 0) {
        setError(
          "Paste a `KChatInstallRequest` JSON payload first. " +
            "This is what the KChat client mints when you join a " +
            "group.",
        );
        return;
      }
      let parsed: KChatInstallRequest;
      try {
        parsed = JSON.parse(trimmed) as KChatInstallRequest;
      } catch (e) {
        setError(
          `Invalid JSON: ${errMsg(e)}. The payload must be a parseable ` +
            "`KChatInstallRequest` (camelCase fields).",
        );
        return;
      }
      const next = await window.kcreate.kchat.install(parsed);
      onStatusChange(next);
      if (next.locked) {
        // Successful round-trip but the bridge re-locked. This
        // happens when the payload's signature is valid but the
        // membership has already expired or is bound to a
        // different peer key.
        setError(
          "The bridge accepted the payload but re-locked the gate. " +
            "Common causes: membership has expired, or it was minted " +
            "for a different peer key than the one this machine uses.",
        );
        onStatus?.("KChat sign-in rejected — see panel for details.");
      } else {
        setPasteJson("");
        onStatus?.(
          `Signed into KChat group ${next.groupId ?? "(unknown)"}.`,
        );
      }
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setBusy(false);
    }
  }, [onStatus, onStatusChange, pasteJson]);

  // ----------------------------------------------------------------
  // Dev mint flow (only available when `devIssuerAvailable`).
  // ----------------------------------------------------------------
  const handleDevMint = useCallback(async (): Promise<void> => {
    setError(null);
    setBusy(true);
    try {
      if (localIdentity === null) {
        setError(
          "Local peer identity is unavailable. Reload the panel " +
            "or check the browser console for the derivation error.",
        );
        return;
      }
      const trimmedGroup = groupId.trim();
      if (trimmedGroup.length === 0) {
        setError("Enter a group id before minting.");
        return;
      }
      const issuerSeed = loadOrGenerateDevIssuerSeed();
      const validForSeconds = Math.max(
        60,
        Math.min(60 * 60 * 24 * 365, Math.round(devValidityHours * 3600)),
      );
      const mintReq: KChatDevMintRequest = {
        issuerSeed,
        groupId: trimmedGroup,
        peerPublicKey: localIdentity.peerPublicKey,
        validForSeconds,
      };
      const install = await window.kcreate.kchat.devMintMembership(mintReq);
      const next = await window.kcreate.kchat.install(install);
      onStatusChange(next);
      try {
        window.localStorage.setItem(LAST_GROUP_ID_STORAGE_KEY, trimmedGroup);
      } catch {
        // Persistence is a nice-to-have; ignore quota errors.
      }
      if (next.locked) {
        setError(
          "Mint succeeded but the gate didn't open. This usually means " +
            "the dev issuer key was rotated mid-flow. Clear the gate and " +
            "retry.",
        );
      } else {
        onStatus?.(
          `Signed into KChat dev group "${trimmedGroup}" via local issuer.`,
        );
      }
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setBusy(false);
    }
  }, [devValidityHours, groupId, localIdentity, onStatus, onStatusChange]);

  // ----------------------------------------------------------------
  // Sign-out flow.
  // ----------------------------------------------------------------
  const handleClear = useCallback(async (): Promise<void> => {
    setError(null);
    setBusy(true);
    try {
      const next = await window.kcreate.kchat.clear();
      onStatusChange(next);
      onStatus?.("KChat sign-out complete; multiplayer is locked.");
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setBusy(false);
    }
  }, [onStatus, onStatusChange]);

  // ----------------------------------------------------------------
  // Render.
  // ----------------------------------------------------------------
  if (!locked && status !== null) {
    return (
      <SignedInView
        status={status}
        onClear={handleClear}
        busy={busy}
        error={error}
        trustedIssuers={trustedIssuers}
        trustedIssuersError={trustedIssuersError}
        onAddTrustedIssuer={handleAddTrustedIssuer}
        onRemoveTrustedIssuer={handleRemoveTrustedIssuer}
      />
    );
  }

  return (
    <section style={sectionStyle}>
      <h3 style={sectionTitleStyle}>Sign into a KChat group</h3>
      <p style={hintStyle}>
        Multiplayer is locked at the protocol layer until a verified
        KChat group membership is installed. The KChat client mints
        an attestation when you join a group; paste it here.
      </p>

      {localIdentity !== null ? (
        <div style={inlineHintStyle}>
          <strong>Your peer id:</strong>
          <code style={codeStyle}>{localIdentity.peerId}</code>
          <strong style={{ marginTop: spacing.xs }}>
            Your peer public key:
          </strong>
          <code style={codeStyle}>{localIdentity.peerPublicKey}</code>
          <span style={hintStyle}>
            The KChat client will need these when minting a membership.
          </span>
        </div>
      ) : null}

      <label style={labelStyle}>
        <span style={labelTextStyle}>Membership attestation (JSON)</span>
        <textarea
          value={pasteJson}
          onChange={(e) => setPasteJson(e.target.value)}
          placeholder='{"issuerPublicKey": "...", "groupId": "...", ...}'
          rows={6}
          spellCheck={false}
          style={textareaStyle}
          disabled={busy}
        />
      </label>
      <div style={buttonRowStyle}>
        <button
          type="button"
          onClick={() => {
            void handleInstall();
          }}
          disabled={busy || pasteJson.trim().length === 0}
          style={primaryButtonStyle(
            busy || pasteJson.trim().length === 0,
          )}
        >
          {busy ? "Installing…" : "Sign in"}
        </button>
      </div>

      {devIssuerAvailable ? (
        <DevMintSection
          groupId={groupId}
          onGroupIdChange={setGroupId}
          validityHours={devValidityHours}
          onValidityHoursChange={setDevValidityHours}
          onMint={handleDevMint}
          busy={busy}
          localIdentityReady={localIdentity !== null}
        />
      ) : null}

      {error !== null ? <div style={errorStyle}>{error}</div> : null}

      <TrustedIssuersSection
        issuers={trustedIssuers}
        error={trustedIssuersError}
        onAdd={handleAddTrustedIssuer}
        onRemove={handleRemoveTrustedIssuer}
      />
    </section>
  );
}

function SignedInView({
  status,
  onClear,
  busy,
  error,
  trustedIssuers,
  trustedIssuersError,
  onAddTrustedIssuer,
  onRemoveTrustedIssuer,
}: {
  status: KChatMembershipStatus;
  onClear: () => void;
  busy: boolean;
  error: string | null;
  trustedIssuers: TrustedIssuer[];
  trustedIssuersError: string | null;
  onAddTrustedIssuer: (input: {
    issuerPublicKey: string;
    label: string;
  }) => Promise<void>;
  onRemoveTrustedIssuer: (issuerPublicKey: string) => Promise<void>;
}): JSX.Element {
  const expiresInLabel = useExpiryCountdown(status.expiresAt);
  // Renderer surface for the trust posture of the active install.
  // The Rust side fills `issuerPublicKey` whenever the gate is
  // open; `issuerLabel` is populated only when the issuer matched
  // a trusted-issuer entry; `issuerTrusted` is `true` when the
  // allowlist is empty (accept-any) OR the issuer matched.
  const issuerPk = status.issuerPublicKey ?? null;
  const issuerLabel = status.issuerLabel ?? null;
  const issuerTrusted = status.issuerTrusted ?? true;
  // Empty allowlist semantics: when the list is empty, every
  // install is "trusted" by default. Differentiate the "truly
  // pinned" UI (badge: Trusted) from the "accept-any" UI (badge:
  // Accept any — not pinned). The renderer copy is explicit so
  // the user understands the trust posture without reading code.
  const allowlistEmpty = trustedIssuers.length === 0;
  return (
    <section style={sectionStyle}>
      <h3 style={sectionTitleStyle}>KChat group</h3>
      <dl style={dlStyle}>
        <dt style={dtStyle}>Group</dt>
        <dd style={ddStyle}>
          <code style={codeStyle}>{status.groupId ?? "(unknown)"}</code>
        </dd>
        <dt style={dtStyle}>Peer id</dt>
        <dd style={ddStyle}>
          <code style={codeStyle}>{status.peerId ?? "(unknown)"}</code>
        </dd>
        <dt style={dtStyle}>Expires</dt>
        <dd style={ddStyle}>
          <code style={codeStyle}>
            {status.expiresAt ?? "(unknown)"}
          </code>
          {expiresInLabel !== null ? (
            <span style={hintStyle}> · {expiresInLabel}</span>
          ) : null}
        </dd>
        <dt style={dtStyle}>Issued by</dt>
        <dd style={ddStyle}>
          {issuerPk !== null ? (
            <>
              <code style={codeStyle}>
                {issuerLabel !== null
                  ? issuerLabel
                  : truncateMiddle(issuerPk, 12)}
              </code>
              <span style={hintStyle}> · </span>
              {issuerTrusted ? (
                allowlistEmpty ? (
                  <span style={infoBadgeStyle}>Accept any — not pinned</span>
                ) : (
                  <span style={trustedBadgeStyle}>Trusted</span>
                )
              ) : (
                <span style={untrustedBadgeStyle}>
                  Untrusted — test only
                </span>
              )}
              {issuerLabel === null && issuerPk !== null ? (
                <div style={hintStyle}>
                  <code style={codeStyle}>{issuerPk}</code>
                </div>
              ) : null}
            </>
          ) : (
            <span style={hintStyle}>(unknown)</span>
          )}
        </dd>
      </dl>
      <div style={buttonRowStyle}>
        <button
          type="button"
          onClick={onClear}
          disabled={busy}
          style={secondaryButtonStyle(busy)}
        >
          {busy ? "Signing out…" : "Sign out"}
        </button>
      </div>
      {error !== null ? <div style={errorStyle}>{error}</div> : null}

      <TrustedIssuersSection
        issuers={trustedIssuers}
        error={trustedIssuersError}
        onAdd={onAddTrustedIssuer}
        onRemove={onRemoveTrustedIssuer}
      />
    </section>
  );
}

/// Trusted-issuer allowlist management. Visible in both the
/// locked and signed-in views so an admin can pre-pin a real
/// KChat issuer before pasting the first attestation OR retire a
/// compromised issuer without signing out first. An empty list
/// is the "accept any issuer" default — explicit in the empty
/// state so the user knows what trust posture they have.
function TrustedIssuersSection({
  issuers,
  error,
  onAdd,
  onRemove,
}: {
  issuers: TrustedIssuer[];
  error: string | null;
  onAdd: (input: { issuerPublicKey: string; label: string }) => Promise<void>;
  onRemove: (issuerPublicKey: string) => Promise<void>;
}): JSX.Element {
  const [pendingPk, setPendingPk] = useState("");
  const [pendingLabel, setPendingLabel] = useState("");
  const [adding, setAdding] = useState(false);
  const canAdd =
    pendingPk.trim().length > 0 && pendingLabel.trim().length > 0 && !adding;
  const handleSubmit = useCallback(async (): Promise<void> => {
    setAdding(true);
    try {
      await onAdd({
        issuerPublicKey: pendingPk.trim(),
        label: pendingLabel.trim(),
      });
      // Only clear inputs on success. `onAdd` re-throws on bridge /
      // validation failure so the user keeps their typed pubkey and
      // label and can fix the issue (e.g. a typo in the base64
      // string) without re-pasting from scratch.
      setPendingPk("");
      setPendingLabel("");
    } catch {
      // Intentional: the error is already surfaced via
      // `trustedIssuersError` by the parent. Swallow here so the
      // promise doesn't trigger an unhandled-rejection warning.
    } finally {
      setAdding(false);
    }
  }, [onAdd, pendingLabel, pendingPk]);
  return (
    <div style={trustSectionStyle}>
      <div style={devHeaderStyle}>
        <strong>Trusted KChat issuers</strong>
        {issuers.length === 0 ? (
          <span style={infoBadgeStyle}>Accept any (empty)</span>
        ) : (
          <span style={trustedBadgeStyle}>
            {issuers.length} pinned
          </span>
        )}
      </div>
      <p style={hintStyle}>
        Pin one or more issuer public keys to require that
        installed memberships come from a known KChat server.
        Leave the list empty to accept any issuer (useful for
        dev / lab work). Persisted under{" "}
        <code style={codeStyle}>kchat_trust.json</code> in the
        Electron user-data directory.
      </p>
      {issuers.length > 0 ? (
        <ul style={trustListStyle}>
          {issuers.map((issuer) => (
            <li key={issuer.issuerPublicKey} style={trustListItemStyle}>
              <div style={trustListItemBodyStyle}>
                <strong>{issuer.label}</strong>
                <code style={codeStyle}>{issuer.issuerPublicKey}</code>
                <span style={hintStyle}>added {issuer.addedAt}</span>
              </div>
              <button
                type="button"
                onClick={() => {
                  void onRemove(issuer.issuerPublicKey);
                }}
                style={secondaryButtonStyle(false)}
              >
                Remove
              </button>
            </li>
          ))}
        </ul>
      ) : null}
      <label style={labelStyle}>
        <span style={labelTextStyle}>Issuer public key</span>
        <input
          type="text"
          value={pendingPk}
          onChange={(e) => setPendingPk(e.target.value)}
          placeholder="URL-safe base64 (no padding required)"
          spellCheck={false}
          style={inputStyle}
          disabled={adding}
        />
      </label>
      <label style={labelStyle}>
        <span style={labelTextStyle}>Label</span>
        <input
          type="text"
          value={pendingLabel}
          onChange={(e) => setPendingLabel(e.target.value)}
          placeholder="e.g. KChat Production"
          spellCheck={false}
          style={inputStyle}
          disabled={adding}
        />
      </label>
      <div style={buttonRowStyle}>
        <button
          type="button"
          onClick={() => {
            void handleSubmit();
          }}
          disabled={!canAdd}
          style={primaryButtonStyle(!canAdd)}
        >
          {adding ? "Adding…" : "Add trusted issuer"}
        </button>
      </div>
      {error !== null ? <div style={errorStyle}>{error}</div> : null}
    </div>
  );
}

/// Visually shorten a long base64 key for the "Issued by" line
/// while keeping enough characters at either end to be
/// recognisable. We only use this when no human-readable label
/// is available. The full pubkey is still shown verbatim below.
function truncateMiddle(s: string, keep: number): string {
  if (s.length <= keep * 2 + 1) return s;
  return `${s.slice(0, keep)}…${s.slice(-keep)}`;
}

function DevMintSection({
  groupId,
  onGroupIdChange,
  validityHours,
  onValidityHoursChange,
  onMint,
  busy,
  localIdentityReady,
}: {
  groupId: string;
  onGroupIdChange: (next: string) => void;
  validityHours: number;
  onValidityHoursChange: (next: number) => void;
  onMint: () => void;
  busy: boolean;
  localIdentityReady: boolean;
}): JSX.Element {
  return (
    <div style={devSectionStyle}>
      <div style={devHeaderStyle}>
        <strong>Dev: mint a membership locally</strong>
        <span style={badgeStyle}>dev-only</span>
      </div>
      <p style={hintStyle}>
        Bridge built with <code>kchat-dev-issuer</code>. Mints a
        membership against an in-process Ed25519 issuer derived
        from a deterministic seed on this machine. Production
        builds do not expose this affordance.
      </p>
      <label style={labelStyle}>
        <span style={labelTextStyle}>Group id</span>
        <input
          type="text"
          value={groupId}
          onChange={(e) => onGroupIdChange(e.target.value)}
          placeholder="kcreate-dev-group"
          spellCheck={false}
          style={inputStyle}
          disabled={busy}
        />
      </label>
      <label style={labelStyle}>
        <span style={labelTextStyle}>Validity (hours)</span>
        <input
          type="number"
          min={1}
          max={24 * 365}
          value={validityHours}
          onChange={(e) => {
            const n = Number(e.target.value);
            if (Number.isFinite(n) && n > 0) onValidityHoursChange(n);
          }}
          style={inputStyle}
          disabled={busy}
        />
      </label>
      <div style={buttonRowStyle}>
        <button
          type="button"
          onClick={onMint}
          disabled={busy || !localIdentityReady || groupId.trim().length === 0}
          style={primaryButtonStyle(
            busy || !localIdentityReady || groupId.trim().length === 0,
          )}
        >
          {busy ? "Minting…" : "Mint dev membership & sign in"}
        </button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Hooks + helpers
// ---------------------------------------------------------------------------

/// Live countdown on the membership expiry. Updates every second
/// so the user sees "expires in 14m 23s" tick down. Returns
/// `null` when no expiry is set (gate is locked or status is
/// stale) or when the wall-clock parse fails.
function useExpiryCountdown(expiresAt: string | null): string | null {
  const [now, setNow] = useState<number>(() => Date.now());
  // Parsed deadline lives in state, not a ref, so `useMemo`'s
  // dependency list can honestly track it. (An earlier version
  // stashed this in a `useRef`, which made the `useMemo`
  // dependencies a lie — when `expiresAt` changed the memo
  // wouldn't recompute until the next 1-second `now` tick.)
  // `useMemo` instead of `useState` here would re-parse every
  // render; `useEffect` ensures we only parse when the input
  // string changes.
  const [deadline, setDeadline] = useState<number | null>(null);
  useEffect(() => {
    if (expiresAt === null) {
      setDeadline(null);
      return;
    }
    const t = Date.parse(expiresAt);
    setDeadline(Number.isFinite(t) ? t : null);
  }, [expiresAt]);

  useEffect(() => {
    if (expiresAt === null) return;
    const handle = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(handle);
  }, [expiresAt]);

  return useMemo(() => {
    if (deadline === null) return null;
    const remaining = Math.max(0, deadline - now);
    return formatDuration(remaining);
  }, [deadline, now]);
}

function formatDuration(ms: number): string {
  if (ms <= 0) return "expired";
  const totalSec = Math.floor(ms / 1000);
  const days = Math.floor(totalSec / 86400);
  const hours = Math.floor((totalSec % 86400) / 3600);
  const mins = Math.floor((totalSec % 3600) / 60);
  const secs = totalSec % 60;
  if (days > 0) return `expires in ${days}d ${hours}h`;
  if (hours > 0) return `expires in ${hours}h ${mins}m`;
  if (mins > 0) return `expires in ${mins}m ${secs}s`;
  return `expires in ${secs}s`;
}

function loadSeedOrNull(): string | null {
  try {
    const existing = window.localStorage.getItem(SEED_STORAGE_KEY);
    if (existing !== null && existing.length > 0) return existing;
  } catch {
    // No localStorage in this environment.
  }
  return null;
}

function loadLastGroupId(): string | null {
  try {
    const existing = window.localStorage.getItem(LAST_GROUP_ID_STORAGE_KEY);
    if (existing !== null && existing.length > 0) return existing;
  } catch {
    // No localStorage in this environment.
  }
  return null;
}

/// Read or generate the per-machine dev issuer seed. Same seed
/// across launches → stable issuer trust root → re-signing in
/// keeps the peer-id-to-issuer binding consistent. We use 32
/// random bytes from `crypto.getRandomValues` (same primitive the
/// PresencePanel uses for the peer seed).
function loadOrGenerateDevIssuerSeed(): string {
  try {
    const existing = window.localStorage.getItem(DEV_ISSUER_SEED_STORAGE_KEY);
    if (existing !== null && existing.length > 0) return existing;
  } catch {
    // Falls through to generate.
  }
  const bytes = new Uint8Array(32);
  window.crypto.getRandomValues(bytes);
  const encoded = base64UrlEncode(bytes);
  try {
    window.localStorage.setItem(DEV_ISSUER_SEED_STORAGE_KEY, encoded);
  } catch {
    // Quota / private mode — return the generated seed anyway;
    // the user will get a new issuer on next launch.
  }
  return encoded;
}

function base64UrlEncode(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return window
    .btoa(bin)
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
}

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

// ---------------------------------------------------------------------------
// Styles
// ---------------------------------------------------------------------------

const sectionStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.sm,
  padding: spacing.sm,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.md,
  background: colors.bgSoft,
};

const sectionTitleStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 11,
  textTransform: "uppercase",
  letterSpacing: 0.4,
  color: colors.textMuted,
};

const hintStyle: React.CSSProperties = {
  fontSize: 11,
  color: colors.textMuted,
  fontStyle: "italic",
  margin: 0,
};

const inlineHintStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 2,
  padding: spacing.xs,
  border: `1px dashed ${colors.border}`,
  borderRadius: radius.sm,
  background: colors.bg,
};

const codeStyle: React.CSSProperties = {
  fontFamily:
    "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
  fontSize: 11,
  color: colors.text,
  wordBreak: "break-all",
};

const labelStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 2,
};

const labelTextStyle: React.CSSProperties = {
  fontSize: 11,
  color: colors.textMuted,
};

const inputStyle: React.CSSProperties = {
  padding: "4px 8px",
  fontSize: 12,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  background: colors.bg,
  color: colors.text,
};

const textareaStyle: React.CSSProperties = {
  ...inputStyle,
  fontFamily:
    "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
  fontSize: 11,
  resize: "vertical",
  minHeight: 80,
};

const buttonRowStyle: React.CSSProperties = {
  display: "flex",
  gap: spacing.sm,
};

function primaryButtonStyle(disabled: boolean): React.CSSProperties {
  return {
    padding: "4px 12px",
    fontSize: 12,
    fontWeight: 500,
    border: "none",
    borderRadius: radius.pill,
    cursor: disabled ? "not-allowed" : "pointer",
    background: disabled ? colors.border : colors.accent,
    color: "#fff",
    opacity: disabled ? 0.6 : 1,
  };
}

function secondaryButtonStyle(disabled: boolean): React.CSSProperties {
  return {
    padding: "4px 12px",
    fontSize: 12,
    fontWeight: 500,
    border: `1px solid ${colors.border}`,
    borderRadius: radius.pill,
    cursor: disabled ? "not-allowed" : "pointer",
    background: colors.bg,
    color: colors.text,
    opacity: disabled ? 0.6 : 1,
  };
}

const devSectionStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.sm,
  padding: spacing.sm,
  border: `1px dashed ${colors.border}`,
  borderRadius: radius.sm,
  background: colors.bg,
};

const devHeaderStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: spacing.sm,
  fontSize: 12,
  color: colors.text,
};

const badgeStyle: React.CSSProperties = {
  fontSize: 10,
  textTransform: "uppercase",
  letterSpacing: 0.4,
  padding: "1px 6px",
  borderRadius: radius.pill,
  background: colors.danger,
  color: "#fff",
};

/// Provenance badges shown on the "Issued by" line in
/// SignedInView. Three distinct visual treatments so the user can
/// tell at a glance whether the install is pinned, accept-any, or
/// untrusted-but-installed (test-only).
const trustedBadgeStyle: React.CSSProperties = {
  fontSize: 10,
  textTransform: "uppercase",
  letterSpacing: 0.4,
  padding: "1px 6px",
  borderRadius: radius.pill,
  background: colors.accent,
  color: "#fff",
};

const untrustedBadgeStyle: React.CSSProperties = {
  fontSize: 10,
  textTransform: "uppercase",
  letterSpacing: 0.4,
  padding: "1px 6px",
  borderRadius: radius.pill,
  background: colors.danger,
  color: "#fff",
};

const infoBadgeStyle: React.CSSProperties = {
  fontSize: 10,
  textTransform: "uppercase",
  letterSpacing: 0.4,
  padding: "1px 6px",
  borderRadius: radius.pill,
  background: colors.border,
  color: colors.text,
};

const trustSectionStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.sm,
  padding: spacing.sm,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  background: colors.bg,
};

const trustListStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.xs,
  margin: 0,
  padding: 0,
  listStyle: "none",
};

const trustListItemStyle: React.CSSProperties = {
  display: "flex",
  gap: spacing.sm,
  alignItems: "flex-start",
  padding: spacing.xs,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  background: colors.bgSoft,
};

const trustListItemBodyStyle: React.CSSProperties = {
  display: "flex",
  flex: 1,
  flexDirection: "column",
  gap: 2,
  minWidth: 0,
};

const dlStyle: React.CSSProperties = {
  display: "grid",
  gridTemplateColumns: "auto 1fr",
  columnGap: spacing.sm,
  rowGap: 4,
  margin: 0,
};

const dtStyle: React.CSSProperties = {
  fontSize: 11,
  color: colors.textMuted,
  alignSelf: "center",
};

const ddStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 12,
  color: colors.text,
  wordBreak: "break-all",
};

const errorStyle: React.CSSProperties = {
  fontSize: 11,
  color: colors.danger,
  padding: spacing.xs,
  border: `1px solid ${colors.danger}`,
  borderRadius: radius.sm,
  background: colors.bg,
};

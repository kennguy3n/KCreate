// @vitest-environment node
//
// Regression test for the collab capability layer in `bridge.ts`.
//
// Every `session_*` and most `kchat_*` N-API exports in
// `crates/kcreate_bridge/src/lib.rs` are gated behind
// `#[cfg(feature = "collab")]`. In a default (non-collab) developer
// build those symbols are ABSENT from the cdylib, so the renderer's
// unconditional `session.info()` / `session.peers()` / `session.locks()`
// polling (six overlays poll on every editor mount) threw
// `TypeError: requireBridge(...).sessionInfo is not a function` back
// through each IPC handler, flooding the main-process log on every
// editor open.
//
// `applyCollabFallbacks` fills only the genuinely-absent collab exports
// with benign "no session" fallbacks (read accessors return idle
// snapshots, fire-and-forget calls no-op, deliberate user actions throw
// one clear error) and otherwise returns a real `collab` build
// untouched. This test pins that contract without a real
// `process.dlopen`.

import { describe, expect, it, vi } from "vitest";

import { applyCollabFallbacks, type Bridge } from "./bridge";

// A representative non-collab cdylib: the always-present exports exist
// (renderer/document plus the ungated `kchat*Available` probes), but
// every `#[cfg(feature = "collab")]` export is missing.
function nonCollabRaw(): {
  raw: Partial<Bridge>;
  documentRequestRender: () => void;
} {
  const documentRequestRender = vi.fn<() => void>();
  const raw: Partial<Bridge> = {
    // Ungated exports that exist in every build.
    documentRequestRender,
    kchatDevIssuerAvailable: () => false,
    kchatBackendAvailable: () => false,
  };
  return { raw, documentRequestRender };
}

describe("applyCollabFallbacks (non-collab build)", () => {
  it("returns no-session snapshots for read accessors", () => {
    const { raw } = nonCollabRaw();
    const bridge = applyCollabFallbacks(raw);

    expect(bridge.sessionInfo()).toBe("null");
    expect(bridge.sessionPeers()).toBe("[]");
    expect(bridge.sessionDrainEvents()).toBe("[]");
    expect(bridge.sessionLocks()).toBe("[]");
    expect(bridge.sessionPendingClipboardOffers()).toBe("[]");
    expect(bridge.sessionLocalPermission()).toBe("editor");
    expect(bridge.sessionAclGet()).toBeNull();
    expect(bridge.sessionKeyEpoch()).toBeNull();
    expect(bridge.sessionFlushPendingOperations()).toBe(0);
    expect(bridge.sessionTickOutboundBatch()).toBe(0);
    expect(bridge.sessionClaimLocks("[]")).toBe("[]");

    // Shapes must survive preload's JSON.parse paths unchanged.
    expect(JSON.parse(bridge.sessionJournalSummary())).toEqual({
      entryCount: 0,
      peerCount: 0,
      byPeer: {},
    });
  });

  it("treats fire-and-forget session calls as inert no-ops", () => {
    const { raw } = nonCollabRaw();
    const bridge = applyCollabFallbacks(raw);

    expect(bridge.sessionLeave()).toBeNull();
    expect(bridge.sessionSendPresence(null, "[]", null)).toBeUndefined();
    expect(bridge.sessionReleaseLocks("[]")).toBeUndefined();
    expect(bridge.sessionQueueOperation("{}")).toBeUndefined();
    expect(bridge.sessionSetActivePages("[]")).toBeUndefined();
    expect(bridge.sessionClipboardReject("offer")).toBeUndefined();
  });

  it("returns locked / empty KChat defaults that parse cleanly", () => {
    const { raw } = nonCollabRaw();
    const bridge = applyCollabFallbacks(raw);

    expect(JSON.parse(bridge.kchatMembershipStatus())).toEqual({
      locked: true,
      groupId: null,
      peerId: null,
      expiresAt: null,
    });
    expect(JSON.parse(bridge.kchatClearAuthority())).toEqual({
      locked: true,
      groupId: null,
      peerId: null,
      expiresAt: null,
    });
    expect(bridge.kchatTrustedIssuers()).toBe("[]");
    // Auto-called on KChatSignInPanel mount when a stored seed exists;
    // "null" → preload JSON.parse → null → setLocalIdentity(null).
    expect(bridge.kchatDeriveLocalIdentity("seed")).toBe("null");
  });

  it("throws ONE clear error for genuinely user-initiated collab actions", () => {
    const { raw } = nonCollabRaw();
    const bridge = applyCollabFallbacks(raw);

    const expectCollabError = (fn: () => unknown): void => {
      expect(fn).toThrow(/collaboration is unavailable in this build/i);
    };

    expectCollabError(() =>
      bridge.sessionStart("seed", "name", "proj", true, null, null),
    );
    expectCollabError(() =>
      bridge.sessionJoin("peer", "pubkey", "name", "127.0.0.1:0", "fp"),
    );
    expectCollabError(() => bridge.sessionKickPeer("peer", "reason"));
    expectCollabError(() => bridge.sessionRequestResume("peer"));
    expectCollabError(() => bridge.sessionSetPeerPermission("peer", "viewer"));
    expectCollabError(() => bridge.sessionAclSet("{}"));
    expectCollabError(() => bridge.sessionRotateKeys(0));
    expectCollabError(() =>
      bridge.sessionClipboardShare("peer", Buffer.from(""), "label"),
    );
    expectCollabError(() => bridge.sessionClipboardAccept("offer"));
    expectCollabError(() => bridge.kchatInstallAuthority("{}"));
    expectCollabError(() => bridge.kchatAddTrustedIssuer("{}"));
    expectCollabError(() => bridge.kchatRemoveTrustedIssuer("key"));
  });

  it("preserves always-present exports instead of shadowing them", () => {
    const { raw, documentRequestRender } = nonCollabRaw();
    const bridge = applyCollabFallbacks(raw);

    bridge.documentRequestRender();
    expect(documentRequestRender).toHaveBeenCalledTimes(1);
    // The ungated availability probes keep returning their real values.
    expect(bridge.kchatBackendAvailable()).toBe(false);
  });

  it("does NOT synthesise exports that main.ts gates with typeof checks", () => {
    const { raw } = nonCollabRaw();
    const bridge = applyCollabFallbacks(raw) as unknown as Record<
      string,
      unknown
    >;

    // Synthesising these would flip main.ts's `typeof fn === "function"`
    // guards and change startup behaviour (trust-store init, dev-mint
    // availability, backend sign-in), so they must stay absent.
    expect(bridge["kchatSetTrustStorePath"]).toBeUndefined();
    expect(bridge["kchatDevMintMembership"]).toBeUndefined();
    expect(bridge["kchatBackendConnect"]).toBeUndefined();
    expect(bridge["kchatBackendListCommunities"]).toBeUndefined();
  });
});

describe("applyCollabFallbacks (collab build)", () => {
  it("returns the raw exports untouched when sessionInfo is present", () => {
    const sessionInfo = vi.fn<() => string>(() => '{"peerId":"abc"}');
    const sessionPeers = vi.fn<() => string>(() => '[{"peerId":"abc"}]');
    const sessionStart = vi.fn<() => string>(() => '{"started":true}');
    const raw: Partial<Bridge> = { sessionInfo, sessionPeers, sessionStart };

    const bridge = applyCollabFallbacks(raw);

    // Identity preserved — no wrapper object allocated for collab builds.
    expect(bridge).toBe(raw as Bridge);
    // Real collab exports are not shadowed by fallbacks.
    expect(bridge.sessionInfo).toBe(sessionInfo);
    expect(bridge.sessionPeers).toBe(sessionPeers);
    expect(bridge.sessionStart).toBe(sessionStart);
    // The real sessionStart runs rather than throwing the fallback error.
    expect(bridge.sessionStart("seed", "name", "proj", true, null, null)).toBe(
      '{"started":true}',
    );
  });
});

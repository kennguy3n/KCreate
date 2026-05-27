// Exercises the host-procedure consumers in `src/store.ts`.
//
// The host bridge is a global injected by the KChat Desktop
// extension runtime (`globalThis.__kchatHost`). The tests stub it
// with an in-memory mock that records the procedure ids being
// called and returns canned payloads.
import { test } from "node:test";
import assert from "node:assert/strict";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));

function makeMockHost(handlers) {
  const calls = [];
  globalThis.__kchatHost = {
    invokeProcedure: async (id, payload) => {
      calls.push({ id, payload });
      const handler = handlers[id];
      if (!handler) {
        return {
          ok: false,
          error: {
            kind: "HOST_PROCEDURE_NOT_FOUND",
            message: `no handler for ${id}`,
          },
        };
      }
      const value = await handler(payload);
      return { ok: true, value };
    },
    openDeeplink: async (url) => {
      calls.push({ id: "deeplink.open_external", payload: { url } });
      return { ok: true };
    },
    subscribe: () => () => {},
  };
  return calls;
}

async function loadStore() {
  const result = await build({
    entryPoints: [resolve(ROOT, "src/store.ts")],
    bundle: true,
    format: "esm",
    target: ["es2022"],
    platform: "neutral",
    write: false,
    legalComments: "none",
    mainFields: ["module", "main"],
    conditions: ["import", "default"],
  });
  const code = result.outputFiles[0]?.text;
  if (!code) {
    throw new Error("failed to compile src/store.ts");
  }
  const dataUrl = `data:text/javascript;base64,${Buffer.from(code).toString(
    "base64",
  )}`;
  return import(dataUrl);
}

test("listMyCommunities maps the response to typed Community[]", async () => {
  const calls = makeMockHost({
    "kchat.query_my_communities": async () => ({
      communities: [
        { id: "c1", name: "Design", role: "admin" },
        { id: "c2", name: "Engineering", role: "member" },
      ],
    }),
  });
  const { listMyCommunities } = await loadStore();
  const out = await listMyCommunities();
  assert.equal(out.length, 2);
  assert.equal(out[0]?.id, "c1");
  assert.equal(out[1]?.role, "member");
  assert.equal(calls[0]?.id, "kchat.query_my_communities");
});

test("listShareInvitesInConversation drops malformed share-invite cards", async () => {
  makeMockHost({
    "kchat.query_messages": async () => ({
      messages: [
        {
          messageId: "m1",
          conversationId: "conv-1",
          senderJid: "alice@kchat",
          contentType: "kcreate.invite.v1",
          content: {
            schemaVersion: 1,
            projectId: "11111111-1111-1111-1111-111111111111",
            projectName: "Mood board",
            ownerPeerId: "peer-1",
            ownerPublicKey: "pk-1",
            ownerDisplayName: "Alice",
            certFingerprint: "fp-1",
            ownerSocketAddr: "192.0.2.5:4433",
            communityId: "comm-1",
            conversationId: "conv-1",
            issuedAt: "2026-05-27T12:00:00Z",
          },
          postedAt: "2026-05-27T12:00:01Z",
        },
        {
          messageId: "m2",
          conversationId: "conv-1",
          senderJid: "bob@kchat",
          contentType: "kcreate.invite.v1",
          // missing required fields → must be dropped.
          content: { schemaVersion: 1, projectId: "00000000-0000-0000-0000-000000000000" },
          postedAt: "2026-05-27T12:00:02Z",
        },
        {
          messageId: "m3",
          conversationId: "conv-1",
          senderJid: "carol@kchat",
          contentType: "text/plain",
          content: "not an invite",
          postedAt: "2026-05-27T12:00:03Z",
        },
      ],
    }),
  });
  const { listShareInvitesInConversation } = await loadStore();
  const out = await listShareInvitesInConversation("conv-1");
  assert.equal(out.length, 1);
  assert.equal(out[0]?.projectName, "Mood board");
  assert.equal(out[0]?.ownerSocketAddr, "192.0.2.5:4433");
});

test("postShareInvite stamps the KCreate content type", async () => {
  const calls = makeMockHost({
    "kchat.post_message": async () => ({
      messageId: "m99",
      postedAt: "2026-05-27T13:00:00Z",
    }),
  });
  const { postShareInvite, KCREATE_SHARE_INVITE_CONTENT_TYPE } = await loadStore();
  await postShareInvite("conv-1", {
    schemaVersion: 1,
    projectId: "11111111-1111-1111-1111-111111111111",
    projectName: "Mood board",
    ownerPeerId: "peer-1",
    ownerPublicKey: "pk-1",
    ownerDisplayName: "Alice",
    certFingerprint: "fp-1",
    ownerSocketAddr: "192.0.2.5:4433",
    communityId: "comm-1",
    conversationId: "conv-1",
    issuedAt: "2026-05-27T12:00:00Z",
  });
  assert.equal(calls[0]?.id, "kchat.post_message");
  assert.equal(
    calls[0]?.payload.contentType,
    KCREATE_SHARE_INVITE_CONTENT_TYPE,
  );
});

test("buildJoinDeeplink embeds the full invite as a base64url payload", async () => {
  const { buildJoinDeeplink } = await loadStore();
  const invite = {
    schemaVersion: 1,
    projectId: "11111111-1111-1111-1111-111111111111",
    projectName: "Mood board",
    ownerPeerId: "peer-1",
    ownerPublicKey: "pk-1",
    ownerDisplayName: "Alice",
    certFingerprint: "fp-1",
    ownerSocketAddr: "192.0.2.5:4433",
    communityId: "comm-1",
    conversationId: "conv-1",
    issuedAt: "2026-05-27T12:00:00Z",
  };
  const url = buildJoinDeeplink(invite);
  assert.ok(url.startsWith("kcreate://join?"));
  const params = new URLSearchParams(url.slice("kcreate://join?".length));
  const payload = params.get("payload");
  assert.ok(payload, "deeplink must carry a payload");
  assert.match(payload, /^[A-Za-z0-9_-]+$/u, "must be base64url-no-pad");
  // Round-trip: decode -> parse -> compare to the input invite.
  const padded = payload + "=".repeat((4 - (payload.length % 4)) % 4);
  const restored = JSON.parse(
    Buffer.from(padded.replaceAll("-", "+").replaceAll("_", "/"), "base64").toString("utf8"),
  );
  assert.deepEqual(restored, invite);
});

test("invokeProcedure throws HostProcedureError when the host denies", async () => {
  makeMockHost({}); // no handler → returns HOST_PROCEDURE_NOT_FOUND
  const { listMyCommunities } = await loadStore();
  await assert.rejects(
    listMyCommunities(),
    /HOST_PROCEDURE_NOT_FOUND/,
  );
});

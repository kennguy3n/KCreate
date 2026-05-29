// Phase 9 Block A Task 6 — unit coverage for the new KChat
// companion-extension components: ProjectBrowserPanel,
// ArtifactCard, SessionStatusBadge, and ActivityFeed.
//
// All four exercise the host-procedure mock pattern already used
// by `tests/store.test.mjs` + `tests/invite-card.test.mjs`. We
// bundle each component (esbuild, react external) into a single
// .mjs file under `dist/test-cache/` so React stays a single
// singleton across the bundled module and the test harness.
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build } from "esbuild";
import { JSDOM } from "jsdom";

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));

function setupDom() {
  const dom = new JSDOM(
    `<!doctype html><html><body><div id="root"></div></body></html>`,
    { pretendToBeVisual: true, url: "http://localhost/" },
  );
  const install = (name, value) => {
    Object.defineProperty(globalThis, name, {
      value,
      writable: true,
      configurable: true,
    });
  };
  install("window", dom.window);
  install("document", dom.window.document);
  install("navigator", dom.window.navigator);
  install("HTMLElement", dom.window.HTMLElement);
  install("Node", dom.window.Node);
  install("Element", dom.window.Element);
  return dom;
}

async function flushReact() {
  for (let i = 0; i < 4; i += 1) {
    await new Promise((r) => setTimeout(r, 0));
  }
}

async function bundleEntry(entry, basename) {
  const cacheDir = resolve(ROOT, "dist/test-cache");
  await mkdir(cacheDir, { recursive: true });
  const outFile = resolve(cacheDir, `${basename}.bundle.mjs`);
  await build({
    entryPoints: [resolve(ROOT, entry)],
    outfile: outFile,
    bundle: true,
    format: "esm",
    target: ["es2022"],
    platform: "neutral",
    legalComments: "none",
    jsx: "automatic",
    external: ["react", "react-dom", "react-dom/*", "react/jsx-runtime"],
    mainFields: ["module", "main"],
    conditions: ["import", "default"],
  });
  return import(pathToFileURL(outFile).href);
}

function makeHostMock(handlers) {
  const calls = [];
  const deeplinks = [];
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
      deeplinks.push(url);
      return { ok: true };
    },
    subscribe: () => () => {},
  };
  return { calls, deeplinks };
}

// ---------------------------------------------------------------
// ProjectBrowserPanel
// ---------------------------------------------------------------

test("ProjectBrowserPanel lists recent projects, sorted newest first", async () => {
  setupDom();
  makeHostMock({
    "kchat.query_my_communities": async () => ({
      communities: [{ id: "c1", name: "Design", role: "member" }],
    }),
    "kchat.query_recent_kcreate_projects": async () => ({
      projects: [
        {
          projectId: "p-old",
          projectName: "Old Poster",
          lastOpenedAt: "2026-05-20T12:00:00Z",
        },
        {
          projectId: "p-new",
          projectName: "New Poster",
          lastOpenedAt: "2026-05-28T12:00:00Z",
        },
      ],
    }),
  });
  const React = await import("react");
  const ReactDOMClient = await import("react-dom/client");
  const { ProjectBrowserPanel } = await bundleEntry(
    "src/ProjectBrowserPanel.tsx",
    "project-browser",
  );

  const container = document.getElementById("root");
  const root = ReactDOMClient.createRoot(container);
  root.render(React.createElement(ProjectBrowserPanel, {}));
  await flushReact();
  const cards = container.querySelectorAll(
    '[data-testid="kcreate-project-card"]',
  );
  assert.equal(cards.length, 2);
  // Newest first.
  assert.equal(cards[0].getAttribute("data-project-id"), "p-new");
  assert.equal(cards[1].getAttribute("data-project-id"), "p-old");
  root.unmount();
});

test("ProjectBrowserPanel dispatches kcreate://open on card click", async () => {
  setupDom();
  makeHostMock({
    "kchat.query_my_communities": async () => ({
      communities: [{ id: "c1", name: "Design", role: "member" }],
    }),
    "kchat.query_recent_kcreate_projects": async () => ({
      projects: [
        {
          projectId: "p-1",
          projectName: "Poster",
          lastOpenedAt: "2026-05-28T12:00:00Z",
        },
      ],
    }),
  });
  const React = await import("react");
  const ReactDOMClient = await import("react-dom/client");
  const { ProjectBrowserPanel } = await bundleEntry(
    "src/ProjectBrowserPanel.tsx",
    "project-browser",
  );

  const container = document.getElementById("root");
  const root = ReactDOMClient.createRoot(container);
  let openedId = null;
  root.render(
    React.createElement(ProjectBrowserPanel, {
      onOpen: async (projectId) => {
        openedId = projectId;
      },
    }),
  );
  await flushReact();
  const card = container.querySelector(
    '[data-testid="kcreate-project-card"] button',
  );
  assert.ok(card);
  card.dispatchEvent(
    new window.MouseEvent("click", { bubbles: true, cancelable: true }),
  );
  await flushReact();
  assert.equal(openedId, "p-1");
  root.unmount();
});

// ---------------------------------------------------------------
// ArtifactCard
// ---------------------------------------------------------------

const ARTIFACT = {
  schemaVersion: 1,
  artifactId: "a-1",
  projectId: "p-1",
  projectName: "Coffee Poster",
  artifactName: "hero.png",
  format: "png",
  byteSize: 1024 * 1024 + 512,
  exportedAt: "2026-05-28T12:00:00Z",
};

test("ArtifactCard renders artifact name and format badge", async () => {
  setupDom();
  makeHostMock({});
  const React = await import("react");
  const ReactDOMClient = await import("react-dom/client");
  const { ArtifactCard } = await bundleEntry(
    "src/ArtifactCard.tsx",
    "artifact-card",
  );

  const container = document.getElementById("root");
  const root = ReactDOMClient.createRoot(container);
  root.render(React.createElement(ArtifactCard, { artifact: ARTIFACT }));
  await flushReact();
  const text = container.textContent ?? "";
  assert.match(text, /hero\.png/);
  assert.match(text, /PNG/);
  assert.match(text, /Coffee Poster/);
  root.unmount();
});

test("ArtifactCard click invokes the onOpen callback", async () => {
  setupDom();
  makeHostMock({});
  const React = await import("react");
  const ReactDOMClient = await import("react-dom/client");
  const { ArtifactCard } = await bundleEntry(
    "src/ArtifactCard.tsx",
    "artifact-card",
  );

  const container = document.getElementById("root");
  const root = ReactDOMClient.createRoot(container);
  let received = null;
  root.render(
    React.createElement(ArtifactCard, {
      artifact: ARTIFACT,
      onOpen: async (a) => {
        received = a;
      },
    }),
  );
  await flushReact();
  const button = container.querySelector(
    '[data-testid="kcreate-artifact-open"]',
  );
  assert.ok(button);
  button.dispatchEvent(
    new window.MouseEvent("click", { bubbles: true, cancelable: true }),
  );
  await flushReact();
  assert.ok(received);
  assert.equal(received.artifactId, "a-1");
  root.unmount();
});

test("buildArtifactDeeplink encodes id + project_id", async () => {
  setupDom();
  makeHostMock({});
  const { buildArtifactDeeplink } = await bundleEntry(
    "src/ArtifactCard.tsx",
    "artifact-card",
  );
  const url = buildArtifactDeeplink(ARTIFACT);
  assert.ok(url.startsWith("kcreate://artifact?"));
  assert.ok(url.includes("id=a-1"));
  assert.ok(url.includes("project_id=p-1"));
});

// ---------------------------------------------------------------
// SessionStatusBadge
// ---------------------------------------------------------------

test("SessionStatusBadge shows installed when probe succeeds", async () => {
  setupDom();
  makeHostMock({});
  const React = await import("react");
  const ReactDOMClient = await import("react-dom/client");
  const { SessionStatusBadge } = await bundleEntry(
    "src/SessionStatusBadge.tsx",
    "session-status",
  );

  const container = document.getElementById("root");
  const root = ReactDOMClient.createRoot(container);
  root.render(
    React.createElement(SessionStatusBadge, {
      pollIntervalMs: Infinity,
      probe: async () => ({ kind: "installed", peerCount: 0 }),
    }),
  );
  await flushReact();
  const badge = container.querySelector('[data-testid="kcreate-session-badge"]');
  assert.ok(badge);
  assert.equal(badge.getAttribute("data-status"), "installed");
  root.unmount();
});

test("SessionStatusBadge shows not-detected on EXTENSION_CAPABILITY_DENIED", async () => {
  setupDom();
  makeHostMock({});
  const React = await import("react");
  const ReactDOMClient = await import("react-dom/client");
  const { SessionStatusBadge } = await bundleEntry(
    "src/SessionStatusBadge.tsx",
    "session-status",
  );

  const container = document.getElementById("root");
  const root = ReactDOMClient.createRoot(container);
  root.render(
    React.createElement(SessionStatusBadge, {
      pollIntervalMs: Infinity,
      probe: async () => ({ kind: "not-detected" }),
    }),
  );
  await flushReact();
  const badge = container.querySelector('[data-testid="kcreate-session-badge"]');
  assert.ok(badge);
  assert.equal(badge.getAttribute("data-status"), "not-detected");
  root.unmount();
});

// ---------------------------------------------------------------
// ActivityFeed
// ---------------------------------------------------------------

test("ActivityFeed surfaces invite + artifact messages newest first", async () => {
  setupDom();
  makeHostMock({
    "kchat.query_messages": async () => ({
      messages: [
        {
          messageId: "m-invite",
          conversationId: "conv-1",
          senderJid: "alice@kchat",
          contentType: "kcreate.invite.v1",
          postedAt: "2026-05-28T12:00:00Z",
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
            issuedAt: "2026-05-28T11:00:00Z",
          },
        },
        {
          messageId: "m-art",
          conversationId: "conv-1",
          senderJid: "bob@kchat",
          contentType: "kcreate.artifact.v1",
          postedAt: "2026-05-28T13:00:00Z",
          content: {
            schemaVersion: 1,
            artifactId: "a-2",
            projectId: "p-2",
            projectName: "Poster",
            artifactName: "hero.png",
            format: "png",
            byteSize: 2048,
            exportedAt: "2026-05-28T12:30:00Z",
          },
        },
        {
          messageId: "m-other",
          conversationId: "conv-1",
          senderJid: "carol@kchat",
          contentType: "text/plain",
          postedAt: "2026-05-28T14:00:00Z",
          content: "ignore me",
        },
      ],
    }),
  });
  const React = await import("react");
  const ReactDOMClient = await import("react-dom/client");
  const { ActivityFeed } = await bundleEntry(
    "src/ActivityFeed.tsx",
    "activity-feed",
  );

  const container = document.getElementById("root");
  const root = ReactDOMClient.createRoot(container);
  root.render(
    React.createElement(ActivityFeed, {
      conversationId: "conv-1",
    }),
  );
  await flushReact();
  const entries = container.querySelectorAll(
    '[data-testid="kcreate-activity-entry"]',
  );
  assert.equal(entries.length, 2);
  // Most recent first.
  assert.equal(entries[0].getAttribute("data-kind"), "artifact");
  assert.equal(entries[1].getAttribute("data-kind"), "invite");
  root.unmount();
});

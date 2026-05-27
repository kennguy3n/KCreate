// Verifies that the InviteCard React component:
//   - Renders the invite project name + owner display name.
//   - Builds a `kcreate://join?...` deeplink visible to the user.
//   - Disables the "Open in KCreate" button when the invite has
//     expired (the card stays visible so the user can audit it,
//     but the action is blocked).
//   - Calls the `onOpen` callback with the invite when the user
//     clicks the button.
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build } from "esbuild";
import { JSDOM } from "jsdom";

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));

const INVITE = {
  schemaVersion: 1,
  projectId: "11111111-1111-1111-1111-111111111111",
  projectName: "Mood board",
  ownerPeerId: "peer-1",
  ownerPublicKey: "pk-1",
  ownerDisplayName: "Alice Liddell",
  certFingerprint: "fp-1",
  ownerSocketAddr: "192.0.2.5:4433",
  communityId: "comm-1",
  conversationId: "conv-1",
  issuedAt: "2026-05-27T12:00:00Z",
};

async function loadInviteCard() {
  // Bundle the component + its store dependency into a single ESM
  // file so the test can import it. React/ReactDOM are marked
  // external so the bundled output uses the same singleton the
  // test process loads through Node's require resolution —
  // otherwise `createRoot` and the imported `useState` come from
  // different React copies and the hooks dispatcher misbehaves.
  //
  // We write the bundle to a real file path under the package's
  // `dist/test-cache/` directory rather than a `data:` URL so the
  // bare `react` / `react-dom` specifiers in the bundled code
  // resolve through the normal node_modules walk. (Node's ESM
  // loader does not perform bare-specifier resolution from `data:`
  // URLs.)
  const cacheDir = resolve(ROOT, "dist/test-cache");
  await mkdir(cacheDir, { recursive: true });
  const outFile = resolve(cacheDir, "invite-card.bundle.mjs");
  await build({
    entryPoints: [resolve(ROOT, "src/InviteCard.tsx")],
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

function setupDom() {
  const dom = new JSDOM(`<!doctype html><html><body><div id="root"></div></body></html>`, {
    pretendToBeVisual: true,
    url: "http://localhost/",
  });
  // Node 22+ makes some globals (notably `navigator`) read-only,
  // so use `Object.defineProperty` with `configurable: true` so
  // subsequent tests can reinstall the jsdom view.
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

// Drain pending React 18 commits scheduled by `createRoot.render`.
// `act()` is the canonical helper but pulling it in here requires
// `@testing-library/react`'s React-specific bundle to be loaded by
// the same React instance the bundle imports, which is brittle on
// Node `node --test`. Three setTimeout-zero yields are reliably
// enough to let React schedule + commit + flush effects.
async function flushReact() {
  for (let i = 0; i < 3; i += 1) {
    await new Promise((r) => setTimeout(r, 0));
  }
}

test("renders project name + owner + a kcreate://join deeplink", async () => {
  setupDom();
  const React = await import("react");
  const ReactDOMClient = await import("react-dom/client");
  const { InviteCard } = await loadInviteCard();

  const container = document.getElementById("root");
  const root = ReactDOMClient.createRoot(container);
  root.render(React.createElement(InviteCard, { invite: INVITE }));
  await flushReact();

  const text = container.textContent ?? "";
  assert.match(text, /Mood board/);
  assert.match(text, /Alice Liddell/);
  const link = container.querySelector("[data-testid=\"kcreate-invite-link\"]");
  assert.ok(link, "deeplink anchor must be rendered");
  assert.ok(link.getAttribute("href").startsWith("kcreate://join?"));
});

test("disables the Open button once the invite has fallen outside the freshness window", async () => {
  setupDom();
  const React = await import("react");
  const ReactDOMClient = await import("react-dom/client");
  const { InviteCard } = await loadInviteCard();

  const container = document.getElementById("root");
  const root = ReactDOMClient.createRoot(container);
  // Pin clock 24 h past issuance with the default 60 min window
  // — invite is firmly outside the freshness window.
  root.render(
    React.createElement(InviteCard, {
      invite: INVITE,
      now: () => new Date("2026-05-28T12:00:00Z"),
    }),
  );
  await flushReact();
  const button = container.querySelector("[data-testid=\"kcreate-invite-open\"]");
  assert.ok(button);
  assert.equal(button.disabled, true);
});

test("calls onOpen with the invite when the user clicks Open", async () => {
  setupDom();
  const React = await import("react");
  const ReactDOMClient = await import("react-dom/client");
  const { InviteCard } = await loadInviteCard();

  const container = document.getElementById("root");
  const root = ReactDOMClient.createRoot(container);

  let received;
  root.render(
    React.createElement(InviteCard, {
      invite: INVITE,
      // Pin clock pre-expiry so the button is enabled.
      now: () => new Date("2026-05-27T12:30:00Z"),
      onOpen: async (invite) => {
        received = invite;
      },
    }),
  );
  await flushReact();

  const button = container.querySelector("[data-testid=\"kcreate-invite-open\"]");
  assert.ok(button);
  button.dispatchEvent(
    new window.MouseEvent("click", { bubbles: true, cancelable: true }),
  );
  // Wait for the async onOpen handler to run.
  await flushReact();
  assert.equal(received?.inviteId, INVITE.inviteId);
});

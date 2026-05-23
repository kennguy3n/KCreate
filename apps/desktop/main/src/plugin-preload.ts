// plugin-preload.ts — preload for sandboxed JS panel `BrowserView`s.
//
// This file runs inside the panel's renderer process (NOT the main
// editor renderer). It is the entire surface a JS panel plugin can
// see — no `require`, no `process`, no filesystem, no `ipcRenderer`.
// The only thing exposed is `window.kcreatePlugin`:
//
//   window.kcreatePlugin.sendMessage(message)
//     → Promise<JsPanelMessageOutcome>
//   window.kcreatePlugin.onMessage(callback)
//     → unsubscribe function for host-pushed messages
//
// The Electron main process wires every `BrowserView` it creates for
// a JS panel with this preload. The preload talks to the main process
// via a single, well-known IPC channel (`kcreate/plugin/js/panel/*`);
// the main process forwards to the bridge after stamping in the
// panel's `pluginId` (which is *NOT* trusted from the panel — the
// host knows which view sent the message).
//
// Security stance:
//   * `sandbox: true`           — chromium sandbox is on
//   * `contextIsolation: true`  — the plugin can't reach into the host
//   * `nodeIntegration: false`  — no Node.js APIs in the panel
//   * Only `sendMessage` / `onMessage` are exposed via contextBridge
//   * The plugin can post anything, but the bridge validates the
//     shape and the panel's declared permissions before any effect.
//
// CSP and network policy are configured at the BrowserView creation
// site in `main.ts`; this file is the in-process gate.

import { contextBridge, ipcRenderer } from "electron";
import type { IpcRendererEvent } from "electron";

// The bridge wire-format is shared with the main renderer through
// `apps/desktop/shared/scene.ts`. We import only the JS panel types
// here so this preload doesn't pull in the entire scene graph type
// surface.
import type {
  JsPanelMessage,
  JsPanelMessageOutcome,
} from "../../shared/scene";

/**
 * Send a structured message from the panel to the bridge. The main
 * process forwards it to `phase2::plugin_js_message`, which validates
 * the message shape and the panel's permissions and (for
 * `write_proposal`) applies the side effect.
 *
 * Returns the bridge's outcome verbatim. The panel uses this to
 * decide whether to update its UI or surface an error.
 */
async function sendMessage(
  message: JsPanelMessage,
): Promise<JsPanelMessageOutcome> {
  const raw = (await ipcRenderer.invoke(
    "kcreate/plugin/js/panel/send",
    JSON.stringify(message),
  )) as string;
  return JSON.parse(raw) as JsPanelMessageOutcome;
}

/**
 * Subscribe to host-pushed messages. The host can push notifications
 * (e.g. "the selection changed") that the panel can react to without
 * polling. The payload is intentionally `unknown` — Phase 2 ships
 * with no push messages defined; this exists so the contract is
 * stable when push semantics arrive in Phase 3 collaboration work.
 *
 * Returns an unsubscribe function. Panels should call this when they
 * unmount to avoid leaking listeners.
 */
function onMessage(
  callback: (payload: unknown) => void,
): () => void {
  const listener = (_event: IpcRendererEvent, payload: unknown) => {
    callback(payload);
  };
  ipcRenderer.on("kcreate/plugin/js/panel/recv", listener);
  return () => {
    ipcRenderer.removeListener("kcreate/plugin/js/panel/recv", listener);
  };
}

contextBridge.exposeInMainWorld("kcreatePlugin", {
  sendMessage,
  onMessage,
});

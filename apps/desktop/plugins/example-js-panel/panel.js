// panel.js — vanilla JS plugin panel.
//
// This plugin asks the bridge for the document's node id list (via
// the read_document permission) and renders it as a flat list. It is
// intentionally minimal: no bundler, no framework, no dependencies.
// Plugin authors can use this as a starting template.
//
// Communication contract (provided by `plugin-preload.ts`):
//
//   window.kcreatePlugin.sendMessage(message)
//     → Promise<JsPanelMessageOutcome>
//   window.kcreatePlugin.onMessage(callback)
//     → unsubscribe function for host push notifications
//
// Message shapes (must match `kcreate_plugin::JsPanelMessage`):
//
//   { type: "read_document",  query: <DocumentQuery JSON> }
//   { type: "write_proposal", proposal: <ProposedMutation JSON> }
//   { type: "log",            message: <string> }
//
// Outcome shapes (`kcreate_plugin::JsPanelMessageOutcome`):
//
//   { status: "ok",       result: <any>      }
//   { status: "denied",   permission: <name> }
//   { status: "invalid",  reason: <string>   }

(() => {
  const treeEl = document.getElementById("tree");
  const refreshEl = document.getElementById("refresh");
  if (!treeEl || !refreshEl) return;

  // The preload script exposes the channel on window.kcreatePlugin.
  // If we're loaded outside the sandbox (e.g. in a browser for
  // testing), it won't be defined — show a friendly message.
  const channel = /** @type {{ sendMessage: (m: unknown) => Promise<unknown> } | undefined} */ (
    window.kcreatePlugin
  );
  if (!channel) {
    treeEl.className = "error";
    treeEl.textContent =
      "kcreatePlugin bridge missing — this panel must run inside KCreate.";
    refreshEl.disabled = true;
    return;
  }

  async function refresh() {
    treeEl.className = "empty";
    treeEl.textContent = "loading…";
    let outcome;
    try {
      outcome = await channel.sendMessage({
        type: "read_document",
        query: { type: "list_nodes" },
      });
    } catch (err) {
      treeEl.className = "error";
      treeEl.textContent = "send failed: " + String(err);
      return;
    }
    if (!outcome || typeof outcome !== "object") {
      treeEl.className = "error";
      treeEl.textContent = "malformed outcome";
      return;
    }
    if (outcome.status === "denied") {
      treeEl.className = "error";
      treeEl.textContent =
        "denied: missing permission " + String(outcome.permission);
      return;
    }
    if (outcome.status === "invalid") {
      treeEl.className = "error";
      treeEl.textContent = "invalid: " + String(outcome.reason);
      return;
    }
    if (outcome.status !== "ok") {
      treeEl.className = "error";
      treeEl.textContent = "unknown status: " + String(outcome.status);
      return;
    }
    const ids = outcome.result;
    if (!Array.isArray(ids) || ids.length === 0) {
      treeEl.className = "empty";
      treeEl.textContent = "(no nodes)";
      return;
    }
    treeEl.className = "";
    const ul = document.createElement("ul");
    for (const id of ids) {
      const li = document.createElement("li");
      const span = document.createElement("span");
      span.className = "id";
      span.textContent = String(id);
      li.appendChild(span);
      ul.appendChild(li);
    }
    treeEl.replaceChildren(ul);
  }

  refreshEl.addEventListener("click", () => {
    void refresh();
  });
  void refresh();
})();

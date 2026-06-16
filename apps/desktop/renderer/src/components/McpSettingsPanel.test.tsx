// McpSettingsPanel tests (I5 — MCP automation server, permission UX).
//
// Pins the governance behaviours the workstream spec calls for:
//   * the master switch reflects `mcpPermission.masterEnabled()` and
//     toggling it routes to `mcpPermission.setMasterEnabled(next)`;
//   * a queued tool call surfaces in the approval inbox, and choosing
//     "Always allow" records it through `mcpPermission.grant(client,
//     tool, "always")` — the path that unblocks the agent's call;
//   * granted scopes render grouped by client and "Revoke" routes to
//     `mcpPermission.revoke(client, tool)`.
//
// Bridge calls are recorded by the session-wide stub installed in
// `setup.vitest.ts`; the `mcp` / `mcpPermission` namespaces were added
// to the stub for this panel.

import { describe, it, expect } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";

import { McpSettingsPanel } from "./McpSettingsPanel";
import { kcreateStub } from "../../tests/helpers/kcreateStub";
import type {
  McpPendingRequest,
  McpPermission,
  McpStatus,
} from "../../../shared/scene";

async function flushAsync(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

const RUNNING: McpStatus = { running: true, port: 51234 };

const PENDING: McpPendingRequest[] = [
  {
    client_id: "demo-agent",
    tool_name: "apply_template",
    first_requested_at: new Date().toISOString(),
    last_requested_at: new Date().toISOString(),
    attempts: 2,
  },
];

const GRANTS: McpPermission[] = [
  {
    client_id: "demo-agent",
    tool_name: "create_node",
    granted: "always",
    granted_at: new Date().toISOString(),
  },
];

describe("McpSettingsPanel", () => {
  it("renders the master switch and toggles it through the bridge", async () => {
    const stub = kcreateStub();
    stub.override("mcpPermission.masterEnabled", () => true);

    render(<McpSettingsPanel />);
    await flushAsync();

    const sw = screen.getByRole("switch", { name: /allow mcp automation/i });
    expect(sw.getAttribute("aria-checked")).toBe("true");

    fireEvent.click(sw);
    await flushAsync();

    const call = stub.calls.find(
      (c) => c.method === "mcpPermission.setMasterEnabled",
    );
    expect(call).toBeDefined();
    expect(call?.args[0]).toBe(false);
  });

  it("surfaces a queued tool call and 'Always allow' records a grant", async () => {
    const stub = kcreateStub();
    stub.override("mcpPermission.status", () => RUNNING);
    stub.override("mcpPermission.pendingList", () => PENDING);

    render(<McpSettingsPanel />);
    await flushAsync();

    // The inbox shows the queued tool + client and the "asked 2×" hint.
    expect(screen.getByText("apply_template")).toBeTruthy();
    expect(screen.getByText(/asked 2/)).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: /always allow/i }));
    await flushAsync();

    const grant = stub.calls.find((c) => c.method === "mcpPermission.grant");
    expect(grant).toBeDefined();
    expect(grant?.args).toEqual(["demo-agent", "apply_template", "always"]);
  });

  it("lists granted scopes grouped by client and revokes them", async () => {
    const stub = kcreateStub();
    stub.override("mcpPermission.list", () => GRANTS);

    render(<McpSettingsPanel />);
    await flushAsync();

    expect(screen.getByText("demo-agent")).toBeTruthy();
    expect(screen.getByText("create_node")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: /revoke/i }));
    await flushAsync();

    const revoke = stub.calls.find((c) => c.method === "mcpPermission.revoke");
    expect(revoke).toBeDefined();
    expect(revoke?.args).toEqual(["demo-agent", "create_node"]);
  });
});

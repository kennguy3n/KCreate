// PluginManager — list installed WASM / JsPanel / Native plugins from
// the local plugin directory, toggle them, and inspect their declared
// permissions. Driven by `window.kcreate.plugin`.

import { useCallback, useEffect, useState } from "react";

import type {
  PluginListEntry,
  PluginPermission,
  PluginType,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface PluginManagerProps {
  onStatus?: (msg: string | null) => void;
}

export function PluginManager({ onStatus }: PluginManagerProps): JSX.Element {
  const [items, setItems] = useState<PluginListEntry[]>([]);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const list = await window.kcreate.plugin.list();
      setItems(list);
    } catch (e) {
      onStatus?.(`plugins: ${errMsg(e)}`);
    }
  }, [onStatus]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const toggle = useCallback(
    async (id: string, enabled: boolean) => {
      setBusy(true);
      try {
        if (enabled) {
          await window.kcreate.plugin.disable(id);
        } else {
          await window.kcreate.plugin.enable(id);
        }
        await refresh();
      } catch (e) {
        onStatus?.(`plugin toggle failed: ${errMsg(e)}`);
      } finally {
        setBusy(false);
      }
    },
    [onStatus, refresh],
  );

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.md,
      }}
    >
      <header
        style={{
          display: "flex",
          alignItems: "center",
          gap: spacing.sm,
        }}
      >
        <h2 style={{ margin: 0, fontSize: 14, fontWeight: 600 }}>Plugins</h2>
        <button
          type="button"
          onClick={() => {
            void refresh();
          }}
          style={{
            marginLeft: "auto",
            padding: "4px 10px",
            background: "transparent",
            border: `1px solid ${colors.border}`,
            borderRadius: radius.pill,
            cursor: "pointer",
            fontSize: 11,
            color: colors.textMuted,
          }}
        >
          Refresh
        </button>
      </header>
      {items.length === 0 ? (
        <p style={{ margin: 0, fontSize: 12, color: colors.textMuted }}>
          No plugins found. Drop a folder containing <code>manifest.json</code>{" "}
          into the plugin directory to get started.
        </p>
      ) : (
        <ul
          style={{
            margin: 0,
            padding: 0,
            listStyle: "none",
            display: "flex",
            flexDirection: "column",
            gap: spacing.sm,
          }}
        >
          {items.map((item) => (
            <PluginCard
              key={item.id}
              item={item}
              busy={busy}
              onToggle={() => {
                void toggle(item.id, item.enabled);
              }}
            />
          ))}
        </ul>
      )}
    </div>
  );
}

function PluginCard({
  item,
  busy,
  onToggle,
}: {
  item: PluginListEntry;
  busy: boolean;
  onToggle: () => void;
}): JSX.Element {
  return (
    <li
      style={{
        padding: spacing.md,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.card,
        background: colors.bg,
        display: "flex",
        flexDirection: "column",
        gap: spacing.sm,
      }}
    >
      <header
        style={{ display: "flex", alignItems: "center", gap: spacing.sm }}
      >
        <h3 style={{ margin: 0, fontSize: 13, fontWeight: 600 }}>
          {item.name}
        </h3>
        <TypePill type={item.type} />
        <span style={{ fontSize: 11, color: colors.textMuted }}>
          v{item.version}
        </span>
        <button
          type="button"
          onClick={onToggle}
          disabled={busy}
          style={{
            marginLeft: "auto",
            padding: "4px 10px",
            background: item.enabled ? colors.accent : "transparent",
            color: item.enabled ? colors.textInverse : colors.text,
            border: item.enabled ? "none" : `1px solid ${colors.border}`,
            borderRadius: radius.pill,
            cursor: busy ? "default" : "pointer",
            fontSize: 11,
            fontWeight: 600,
          }}
        >
          {item.enabled ? "Disable" : "Enable"}
        </button>
      </header>
      {item.author ? (
        <p
          style={{
            margin: 0,
            fontSize: 11,
            color: colors.textMuted,
          }}
        >
          by {item.author}
        </p>
      ) : null}
      {item.description ? (
        <p style={{ margin: 0, fontSize: 12, color: colors.text }}>
          {item.description}
        </p>
      ) : null}
      {item.permissions.length > 0 ? (
        <div style={{ display: "flex", gap: 4, flexWrap: "wrap" }}>
          {item.permissions.map((p) => (
            <PermissionPill key={p} permission={p} />
          ))}
        </div>
      ) : null}
    </li>
  );
}

function TypePill({ type }: { type: PluginType }): JSX.Element {
  const label =
    type === "wasm" ? "WASM" : type === "js_panel" ? "JS panel" : "Native";
  const isWasm = type === "wasm";
  return (
    <span
      style={{
        padding: "1px 6px",
        background: isWasm ? `${colors.accent}22` : colors.bgSoft,
        color: isWasm ? colors.accent : colors.textMuted,
        borderRadius: radius.pill,
        fontSize: 10,
        fontWeight: 600,
        textTransform: "uppercase",
        letterSpacing: 0.3,
      }}
    >
      {label}
    </span>
  );
}

function PermissionPill({
  permission,
}: {
  permission: PluginPermission;
}): JSX.Element {
  const isDangerous = permission === "network_access";
  return (
    <span
      style={{
        padding: "1px 6px",
        background: isDangerous ? "#DC262622" : colors.bgSoft,
        color: isDangerous ? "#DC2626" : colors.textMuted,
        borderRadius: radius.pill,
        fontSize: 10,
        fontWeight: 600,
      }}
    >
      {permission.replace(/_/g, " ")}
    </span>
  );
}

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

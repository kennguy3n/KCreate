// PluginManager — list installed WASM / JsPanel / Native plugins from
// the local plugin directory, toggle them, and inspect their declared
// permissions. Driven by `window.kcreate.plugin`.

import { useCallback, useEffect, useState } from "react";

import type {
  PluginListEntry,
  PluginPermission,
  PluginSignatureStatus,
  PluginType,
  TrustedKeyInfo,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface PluginManagerProps {
  onStatus?: (msg: string | null) => void;
}

export function PluginManager({ onStatus }: PluginManagerProps): JSX.Element {
  const [items, setItems] = useState<PluginListEntry[]>([]);
  const [trustedKeys, setTrustedKeys] = useState<TrustedKeyInfo[]>([]);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const [list, trusted] = await Promise.all([
        window.kcreate.plugin.list(),
        window.kcreate.plugin.trustList(),
      ]);
      setItems(list);
      setTrustedKeys(
        [...trusted].sort((a, b) => a.keyId.localeCompare(b.keyId)),
      );
    } catch (e) {
      onStatus?.(`plugins: ${errMsg(e)}`);
    }
  }, [onStatus]);

  const reloadTrust = useCallback(async () => {
    setBusy(true);
    try {
      await window.kcreate.plugin.trustReload();
      await refresh();
      onStatus?.(
        "trust store reloaded — plugins rescanned against trusted_keys.json",
      );
    } catch (e) {
      onStatus?.(`trust reload failed: ${errMsg(e)}`);
    } finally {
      setBusy(false);
    }
  }, [onStatus, refresh]);

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
      <TrustedAuthorities
        keys={trustedKeys}
        busy={busy}
        onReload={() => {
          void reloadTrust();
        }}
      />
    </div>
  );
}

/// "Trusted Authorities" — list of Ed25519 public keys allowed to
/// sign native plugins. Native plugins are rejected at registry-scan
/// time unless they carry a `manifest.json.sig` signed by one of
/// these keys; sandboxed (WASM / js_panel) plugins load regardless
/// but surface their signature status next to each entry.
function TrustedAuthorities({
  keys,
  busy,
  onReload,
}: {
  keys: TrustedKeyInfo[];
  busy: boolean;
  onReload: () => void;
}): JSX.Element {
  return (
    <section
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.sm,
        padding: spacing.md,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.card,
        background: colors.bgSoft,
      }}
    >
      <header
        style={{ display: "flex", alignItems: "center", gap: spacing.sm }}
      >
        <h3 style={{ margin: 0, fontSize: 12, fontWeight: 600 }}>
          Trusted authorities
        </h3>
        <button
          type="button"
          onClick={onReload}
          disabled={busy}
          style={{
            marginLeft: "auto",
            padding: "4px 10px",
            background: "transparent",
            border: `1px solid ${colors.border}`,
            borderRadius: radius.pill,
            cursor: busy ? "default" : "pointer",
            fontSize: 11,
            color: colors.textMuted,
          }}
        >
          Reload
        </button>
      </header>
      {keys.length === 0 ? (
        <p style={{ margin: 0, fontSize: 11, color: colors.textMuted }}>
          No trusted Ed25519 keys are installed. Native plugins are blocked
          until you add at least one key to{" "}
          <code>~/.kcreate/plugins/trusted_keys.json</code> and click Reload.
        </p>
      ) : (
        <ul
          style={{
            margin: 0,
            padding: 0,
            listStyle: "none",
            display: "flex",
            flexDirection: "column",
            gap: 4,
          }}
        >
          {keys.map((k) => (
            <li
              key={k.keyId}
              style={{
                display: "flex",
                alignItems: "baseline",
                gap: spacing.sm,
                fontSize: 11,
              }}
            >
              <code
                style={{
                  fontFamily: "monospace",
                  color: colors.text,
                }}
              >
                {k.keyId}
              </code>
              {k.comment ? (
                <span style={{ color: colors.textMuted }}>{k.comment}</span>
              ) : null}
            </li>
          ))}
        </ul>
      )}
    </section>
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
        <SignaturePill signature={item.signature} />
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
        background: isWasm ? colors.accentBgSoft : colors.bgSoft,
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

function SignaturePill({
  signature,
}: {
  signature: PluginSignatureStatus;
}): JSX.Element {
  const { label, bg, fg, title } = signaturePresentation(signature);
  return (
    <span
      title={title}
      style={{
        padding: "1px 6px",
        background: bg,
        color: fg,
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

/// Visual presentation for the four signature states. Verified is
/// the only "green"; Invalid is red so the user can't miss it on
/// sandboxed (WASM / js_panel) plugins that loaded *despite* a bad
/// signature.
function signaturePresentation(signature: PluginSignatureStatus): {
  label: string;
  bg: string;
  fg: string;
  title: string;
} {
  switch (signature.status) {
    case "verified":
      return {
        label: "Signed",
        bg: "#16A34A22",
        fg: "#16A34A",
        title: `Signed by trusted key "${signature.key_id}"`,
      };
    case "invalid":
      return {
        label: "Bad sig",
        bg: colors.dangerBg,
        fg: colors.danger,
        title: `Signature failed verification (key "${signature.key_id}"): ${signature.reason}`,
      };
    case "unsigned":
    default:
      return {
        label: "Unsigned",
        bg: colors.bgSoft,
        fg: colors.textMuted,
        title:
          "No manifest.json.sig sidecar — native plugins are blocked, sandboxed plugins load anyway.",
      };
  }
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
        background: isDangerous ? colors.dangerBg : colors.bgSoft,
        color: isDangerous ? colors.danger : colors.textMuted,
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

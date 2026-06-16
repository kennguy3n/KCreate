// PluginManager — a browsable in-app plugin gallery driven by
// `window.kcreate.plugin` (installed state, enable/disable, trust,
// run-on-selection) and `window.kcreate.phase10` (the offline
// catalog: bundled + on-disk plugins, install / remove).
//
// The catalog is sourced from a real on-disk/bundled registry — no
// network is touched on this path. Each card surfaces the plugin's
// trust state (signed by a trusted Ed25519 key vs unsigned vs bad
// signature) and the exact host-ABI capabilities it requests.
// Enabling an unsigned or over-broad plugin requires explicit consent
// via a modal; signed plugins with only safe permissions enable in
// one click.

import { useCallback, useEffect, useMemo, useState } from "react";

import type {
  PluginExecuteWithContextResult,
  PluginListEntry,
  PluginListing,
  PluginPermission,
  PluginSignatureStatus,
  PluginType,
  TrustedKeyInfo,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface PluginManagerProps {
  onStatus?: (msg: string | null) => void;
}

/** Browse filter for the gallery. */
type GalleryFilter = "all" | "installed" | "available";

/**
 * Unified view model for one gallery row. Merges the catalog entry
 * (browse + install) with the installed registry entry (enabled state,
 * structured signature, plugin type) when the plugin is installed.
 */
interface GalleryEntry {
  id: string;
  name: string;
  version: string;
  author: string;
  description: string;
  permissions: PluginPermission[];
  installed: boolean;
  enabled: boolean;
  /** Known only once installed (the catalog doesn't carry it). */
  type: PluginType | null;
  signature: PluginSignatureStatus;
}

export function PluginManager({ onStatus }: PluginManagerProps): JSX.Element {
  const [entries, setEntries] = useState<GalleryEntry[]>([]);
  const [trustedKeys, setTrustedKeys] = useState<TrustedKeyInfo[]>([]);
  const [busy, setBusy] = useState(false);
  const [filter, setFilter] = useState<GalleryFilter>("all");
  const [consent, setConsent] = useState<GalleryEntry | null>(null);
  const [removing, setRemoving] = useState<GalleryEntry | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [catalog, installed, trusted] = await Promise.all([
        window.kcreate.phase10.pluginMarketplaceCatalog(),
        window.kcreate.plugin.list(),
        window.kcreate.plugin.trustList(),
      ]);
      setEntries(buildGallery(catalog, installed));
      setTrustedKeys(
        [...trusted].sort((a, b) => a.keyId.localeCompare(b.keyId)),
      );
    } catch (e) {
      onStatus?.(`plugins: ${errMsg(e)}`);
    }
  }, [onStatus]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

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

  const install = useCallback(
    async (entry: GalleryEntry) => {
      setBusy(true);
      try {
        await window.kcreate.phase10.pluginMarketplaceInstallBundled(entry.id);
        await refresh();
        onStatus?.(`installed “${entry.name}” — enable it to run`);
      } catch (e) {
        onStatus?.(`install failed: ${errMsg(e)}`);
      } finally {
        setBusy(false);
      }
    },
    [onStatus, refresh],
  );

  const doEnable = useCallback(
    async (entry: GalleryEntry) => {
      setBusy(true);
      try {
        await window.kcreate.plugin.enable(entry.id);
        await refresh();
        onStatus?.(`enabled “${entry.name}”`);
      } catch (e) {
        onStatus?.(`enable failed: ${errMsg(e)}`);
      } finally {
        setBusy(false);
      }
    },
    [onStatus, refresh],
  );

  const requestEnable = useCallback(
    (entry: GalleryEntry) => {
      if (needsConsent(entry.signature, entry.permissions)) {
        setConsent(entry);
        return;
      }
      void doEnable(entry);
    },
    [doEnable],
  );

  const disable = useCallback(
    async (entry: GalleryEntry) => {
      setBusy(true);
      try {
        await window.kcreate.plugin.disable(entry.id);
        await refresh();
        onStatus?.(`disabled “${entry.name}”`);
      } catch (e) {
        onStatus?.(`disable failed: ${errMsg(e)}`);
      } finally {
        setBusy(false);
      }
    },
    [onStatus, refresh],
  );

  const remove = useCallback(
    async (entry: GalleryEntry) => {
      setBusy(true);
      try {
        await window.kcreate.phase10.pluginMarketplaceRemove(entry.id);
        await refresh();
        onStatus?.(`removed “${entry.name}”`);
      } catch (e) {
        onStatus?.(`remove failed: ${errMsg(e)}`);
      } finally {
        setBusy(false);
        setRemoving(null);
      }
    },
    [onStatus, refresh],
  );

  const run = useCallback(
    async (entry: GalleryEntry) => {
      setBusy(true);
      try {
        const result = await window.kcreate.plugin.executeOnSelection(
          entry.id,
          "run",
          "{}",
        );
        onStatus?.(summariseRun(entry.name, result));
      } catch (e) {
        onStatus?.(`run failed: ${errMsg(e)}`);
      } finally {
        setBusy(false);
      }
    },
    [onStatus],
  );

  const counts = useMemo(() => {
    const installed = entries.filter((e) => e.installed).length;
    return { all: entries.length, installed, available: entries.length - installed };
  }, [entries]);

  const visible = useMemo(
    () =>
      entries.filter((e) =>
        filter === "installed"
          ? e.installed
          : filter === "available"
            ? !e.installed
            : true,
      ),
    [entries, filter],
  );

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: spacing.md }}>
      <header style={{ display: "flex", alignItems: "center", gap: spacing.sm }}>
        <h2 style={{ margin: 0, fontSize: 14, fontWeight: 600 }}>
          Plugin gallery
        </h2>
        <button
          type="button"
          onClick={() => {
            void refresh();
          }}
          style={{ ...pillButton(false), marginLeft: "auto" }}
        >
          Refresh
        </button>
      </header>

      <p style={{ margin: 0, fontSize: 11, color: colors.textMuted }}>
        Offline catalog — bundled & on-disk plugins. Install, then enable to
        run. Plugins run in a WASM sandbox with no file, network, or DOM
        access; they can only change the document through a reviewed proposal.
      </p>

      <FilterTabs filter={filter} counts={counts} onChange={setFilter} />

      {visible.length === 0 ? (
        <p style={{ margin: 0, fontSize: 12, color: colors.textMuted }}>
          {filter === "available"
            ? "Every catalog plugin is already installed."
            : filter === "installed"
              ? "No plugins installed yet. Switch to “Available” to install one."
              : "No plugins found. Drop a folder containing manifest.json into the plugin directory, or install one from the catalog."}
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
          {visible.map((entry) => (
            <PluginCard
              key={entry.id}
              entry={entry}
              busy={busy}
              onInstall={() => {
                void install(entry);
              }}
              onEnable={() => {
                requestEnable(entry);
              }}
              onDisable={() => {
                void disable(entry);
              }}
              onRemove={() => {
                setRemoving(entry);
              }}
              onRun={() => {
                void run(entry);
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

      {consent ? (
        <ConsentDialog
          entry={consent}
          busy={busy}
          onCancel={() => {
            setConsent(null);
          }}
          onConfirm={() => {
            const target = consent;
            setConsent(null);
            void doEnable(target);
          }}
        />
      ) : null}

      {removing ? (
        <RemoveDialog
          entry={removing}
          busy={busy}
          onCancel={() => {
            setRemoving(null);
          }}
          onConfirm={() => {
            void remove(removing);
          }}
        />
      ) : null}
    </div>
  );
}

/**
 * Merge the offline catalog (browse + install + trust string) with the
 * installed registry entries (enabled state, structured signature,
 * plugin type). The catalog is the superset — it lists installed
 * plugins first, then bundled plugins that aren't installed yet — so we
 * iterate it and enrich each row from the installed map when present.
 */
function buildGallery(
  catalog: PluginListing[],
  installed: PluginListEntry[],
): GalleryEntry[] {
  const byId = new Map<string, PluginListEntry>();
  for (const e of installed) byId.set(e.id, e);
  return catalog.map((c) => {
    const inst = byId.get(c.id);
    return {
      id: c.id,
      name: c.name,
      version: c.version,
      author: c.author,
      description: c.description,
      permissions: inst
        ? inst.permissions
        : (c.permissions as PluginPermission[]),
      installed: c.installed || inst !== undefined,
      enabled: inst?.enabled ?? false,
      type: inst?.type ?? null,
      signature: inst?.signature ?? parseTrustStatus(c.trustStatus),
    };
  });
}

/**
 * Parse the catalog's `trustStatus` string (`verified:<key>`,
 * `invalid:<key>:<reason>`, `unsigned`) into the structured
 * `PluginSignatureStatus` the UI renders. Used for catalog-only rows
 * (not-yet-installed plugins) where we don't have the registry's
 * structured signature.
 */
function parseTrustStatus(status: string): PluginSignatureStatus {
  if (status.startsWith("verified:")) {
    return { status: "verified", key_id: status.slice("verified:".length) };
  }
  if (status.startsWith("invalid:")) {
    const rest = status.slice("invalid:".length);
    const sep = rest.indexOf(":");
    if (sep >= 0) {
      return {
        status: "invalid",
        key_id: rest.slice(0, sep),
        reason: rest.slice(sep + 1),
      };
    }
    return { status: "invalid", key_id: rest, reason: "verification failed" };
  }
  return { status: "unsigned" };
}

/**
 * A plugin needs an explicit consent gate before it can be enabled if
 * it is not signed by a trusted key (unsigned / bad signature) OR it
 * requests an over-broad capability (network access — disallowed by the
 * sandbox, so a manifest asking for it is a red flag worth surfacing).
 */
function needsConsent(
  signature: PluginSignatureStatus,
  permissions: PluginPermission[],
): boolean {
  const trusted = signature.status === "verified";
  const overBroad = permissions.includes("network_access");
  return !trusted || overBroad;
}

/** Friendly one-liner for a run result, parsed from the plugin output. */
function summariseRun(
  name: string,
  result: PluginExecuteWithContextResult,
): string {
  const applied = result.proposals.filter(
    (p) => p.outcome.status === "applied",
  ).length;
  const rejected = result.proposals.filter(
    (p) => p.outcome.status === "rejected",
  ).length;
  if (applied === 0 && rejected === 0) {
    return `“${name}” ran but changed nothing — select one or more layers first`;
  }
  const parts = [`“${name}” applied ${applied} change${applied === 1 ? "" : "s"}`];
  if (rejected > 0) parts.push(`${rejected} rejected`);
  return `${parts.join(", ")} — undo (Ctrl/Cmd+Z) reverts it as one step`;
}

function FilterTabs({
  filter,
  counts,
  onChange,
}: {
  filter: GalleryFilter;
  counts: { all: number; installed: number; available: number };
  onChange: (f: GalleryFilter) => void;
}): JSX.Element {
  const tabs: { key: GalleryFilter; label: string; count: number }[] = [
    { key: "all", label: "All", count: counts.all },
    { key: "installed", label: "Installed", count: counts.installed },
    { key: "available", label: "Available", count: counts.available },
  ];
  return (
    <div
      role="tablist"
      aria-label="Filter plugins"
      style={{
        display: "inline-flex",
        gap: 2,
        padding: 2,
        background: colors.bgSoft,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.pill,
        alignSelf: "flex-start",
      }}
    >
      {tabs.map((t) => {
        const active = t.key === filter;
        return (
          <button
            key={t.key}
            type="button"
            role="tab"
            aria-selected={active}
            onClick={() => {
              onChange(t.key);
            }}
            style={{
              padding: "4px 12px",
              border: "none",
              borderRadius: radius.pill,
              cursor: "pointer",
              fontSize: 11,
              fontWeight: 600,
              background: active ? colors.accent : "transparent",
              color: active ? colors.textInverse : colors.textMuted,
            }}
          >
            {t.label}
            <span style={{ marginLeft: 6, opacity: 0.8 }}>{t.count}</span>
          </button>
        );
      })}
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
      <header style={{ display: "flex", alignItems: "center", gap: spacing.sm }}>
        <h3 style={{ margin: 0, fontSize: 12, fontWeight: 600 }}>
          Trusted authorities
        </h3>
        <button
          type="button"
          onClick={onReload}
          disabled={busy}
          style={{ ...pillButton(busy), marginLeft: "auto" }}
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
              <code style={{ fontFamily: "monospace", color: colors.text }}>
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
  entry,
  busy,
  onInstall,
  onEnable,
  onDisable,
  onRemove,
  onRun,
}: {
  entry: GalleryEntry;
  busy: boolean;
  onInstall: () => void;
  onEnable: () => void;
  onDisable: () => void;
  onRemove: () => void;
  onRun: () => void;
}): JSX.Element {
  const canRun = entry.installed && entry.enabled && entry.type === "wasm";
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
      <header style={{ display: "flex", alignItems: "center", gap: spacing.sm }}>
        <h3 style={{ margin: 0, fontSize: 13, fontWeight: 600 }}>{entry.name}</h3>
        {entry.type ? <TypePill type={entry.type} /> : null}
        <SignaturePill signature={entry.signature} />
        <span style={{ fontSize: 11, color: colors.textMuted }}>
          v{entry.version}
        </span>
        <div
          style={{
            marginLeft: "auto",
            display: "flex",
            gap: spacing.sm,
            alignItems: "center",
          }}
        >
          {!entry.installed ? (
            <button
              type="button"
              onClick={onInstall}
              disabled={busy}
              style={solidButton(busy)}
            >
              Install
            </button>
          ) : (
            <>
              {canRun ? (
                <button
                  type="button"
                  onClick={onRun}
                  disabled={busy}
                  style={solidButton(busy)}
                  title="Run this plugin against the current selection"
                >
                  Run on selection
                </button>
              ) : null}
              <button
                type="button"
                onClick={entry.enabled ? onDisable : onEnable}
                disabled={busy}
                style={entry.enabled ? pillButton(busy) : accentButton(busy)}
              >
                {entry.enabled ? "Disable" : "Enable"}
              </button>
              <button
                type="button"
                onClick={onRemove}
                disabled={busy}
                style={dangerGhostButton(busy)}
                title="Uninstall this plugin"
              >
                Remove
              </button>
            </>
          )}
        </div>
      </header>
      {entry.author ? (
        <p style={{ margin: 0, fontSize: 11, color: colors.textMuted }}>
          by {entry.author}
        </p>
      ) : null}
      {entry.description ? (
        <p style={{ margin: 0, fontSize: 12, color: colors.text }}>
          {entry.description}
        </p>
      ) : null}
      {entry.permissions.length > 0 ? (
        <div style={{ display: "flex", gap: 4, flexWrap: "wrap" }}>
          {entry.permissions.map((p) => (
            <PermissionPill key={p} permission={p} />
          ))}
        </div>
      ) : (
        <span style={{ fontSize: 10, color: colors.textMuted }}>
          No host capabilities requested
        </span>
      )}
    </li>
  );
}

/**
 * Consent modal shown before enabling an unsigned / bad-signature /
 * over-broad plugin. Spells out the exact trust gap and every host
 * capability the plugin will be granted so the choice is informed.
 */
function ConsentDialog({
  entry,
  busy,
  onCancel,
  onConfirm,
}: {
  entry: GalleryEntry;
  busy: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}): JSX.Element {
  const reason =
    entry.signature.status === "invalid"
      ? `Its manifest signature failed verification (key “${entry.signature.key_id}”). The signed bytes don't match — the plugin may have been tampered with.`
      : entry.signature.status === "unsigned"
        ? "It is not signed by any trusted authority, so its publisher can't be verified."
        : "It requests an over-broad capability beyond what the sandbox normally grants.";
  return (
    <ModalShell title={`Enable “${entry.name}”?`} onCancel={onCancel}>
      <p style={{ margin: 0, fontSize: 12, color: colors.text }}>{reason}</p>
      <p style={{ margin: 0, fontSize: 12, color: colors.text }}>
        Enabling grants it these host capabilities:
      </p>
      {entry.permissions.length > 0 ? (
        <div style={{ display: "flex", gap: 4, flexWrap: "wrap" }}>
          {entry.permissions.map((p) => (
            <PermissionPill key={p} permission={p} />
          ))}
        </div>
      ) : (
        <span style={{ fontSize: 11, color: colors.textMuted }}>
          None — it can only read the input you hand it.
        </span>
      )}
      <p style={{ margin: 0, fontSize: 11, color: colors.textMuted }}>
        It still runs fully sandboxed: no file, network, or DOM access, and
        every document change goes through a reviewed proposal.
      </p>
      <div
        style={{
          display: "flex",
          gap: spacing.sm,
          justifyContent: "flex-end",
          marginTop: spacing.sm,
        }}
      >
        <button type="button" onClick={onCancel} disabled={busy} style={pillButton(busy)}>
          Cancel
        </button>
        <button
          type="button"
          onClick={onConfirm}
          disabled={busy}
          style={dangerSolidButton(busy)}
        >
          Enable anyway
        </button>
      </div>
    </ModalShell>
  );
}

/** Confirmation modal for uninstalling a plugin. */
function RemoveDialog({
  entry,
  busy,
  onCancel,
  onConfirm,
}: {
  entry: GalleryEntry;
  busy: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}): JSX.Element {
  return (
    <ModalShell title={`Remove “${entry.name}”?`} onCancel={onCancel}>
      <p style={{ margin: 0, fontSize: 12, color: colors.text }}>
        This deletes the plugin from your plugin directory. Bundled plugins can
        be reinstalled from the catalog at any time.
      </p>
      <div
        style={{
          display: "flex",
          gap: spacing.sm,
          justifyContent: "flex-end",
          marginTop: spacing.sm,
        }}
      >
        <button type="button" onClick={onCancel} disabled={busy} style={pillButton(busy)}>
          Cancel
        </button>
        <button
          type="button"
          onClick={onConfirm}
          disabled={busy}
          style={dangerSolidButton(busy)}
        >
          Remove
        </button>
      </div>
    </ModalShell>
  );
}

function ModalShell({
  title,
  onCancel,
  children,
}: {
  title: string;
  onCancel: () => void;
  children: React.ReactNode;
}): JSX.Element {
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={title}
      onClick={onCancel}
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(0, 0, 0, 0.45)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        zIndex: 1000,
        padding: spacing.lg,
      }}
    >
      <div
        onClick={(e) => {
          e.stopPropagation();
        }}
        style={{
          width: "min(420px, 100%)",
          background: colors.bg,
          border: `1px solid ${colors.border}`,
          borderRadius: radius.card,
          padding: spacing.lg,
          display: "flex",
          flexDirection: "column",
          gap: spacing.sm,
          boxShadow: "0 12px 40px rgba(0, 0, 0, 0.35)",
        }}
      >
        <h3 style={{ margin: 0, fontSize: 14, fontWeight: 600 }}>{title}</h3>
        {children}
      </div>
    </div>
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
      title={permissionHint(permission)}
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

function permissionHint(permission: PluginPermission): string {
  switch (permission) {
    case "read_document":
      return "Read the current document's nodes and geometry.";
    case "write_document":
      return "Propose document changes (applied only after host review).";
    case "read_assets":
      return "Read embedded asset bytes by content hash.";
    case "export_files":
      return "Produce export artifacts.";
    case "network_access":
      return "Network access — denied by the sandbox; treat as a red flag.";
    default:
      return permission;
  }
}

// --- shared inline-button styles -------------------------------------------

function pillButton(busy: boolean): React.CSSProperties {
  return {
    padding: "4px 10px",
    background: "transparent",
    border: `1px solid ${colors.border}`,
    borderRadius: radius.pill,
    cursor: busy ? "default" : "pointer",
    fontSize: 11,
    fontWeight: 600,
    color: colors.textMuted,
  };
}

function accentButton(busy: boolean): React.CSSProperties {
  return {
    padding: "4px 10px",
    background: "transparent",
    border: `1px solid ${colors.accent}`,
    borderRadius: radius.pill,
    cursor: busy ? "default" : "pointer",
    fontSize: 11,
    fontWeight: 600,
    color: colors.accent,
  };
}

function solidButton(busy: boolean): React.CSSProperties {
  return {
    padding: "4px 10px",
    background: colors.accent,
    border: "none",
    borderRadius: radius.pill,
    cursor: busy ? "default" : "pointer",
    fontSize: 11,
    fontWeight: 600,
    color: colors.textInverse,
  };
}

function dangerGhostButton(busy: boolean): React.CSSProperties {
  return {
    padding: "4px 10px",
    background: "transparent",
    border: `1px solid ${colors.border}`,
    borderRadius: radius.pill,
    cursor: busy ? "default" : "pointer",
    fontSize: 11,
    fontWeight: 600,
    color: colors.danger,
  };
}

function dangerSolidButton(busy: boolean): React.CSSProperties {
  return {
    padding: "4px 12px",
    background: colors.danger,
    border: "none",
    borderRadius: radius.pill,
    cursor: busy ? "default" : "pointer",
    fontSize: 11,
    fontWeight: 600,
    color: "#fff",
  };
}

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

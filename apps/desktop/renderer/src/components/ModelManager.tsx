// ModelManager — local AI model lifecycle UI.
//
// Surfaces:
//   - currently-loaded model (name, status, listening port, device tier)
//   - effective max-model-size budget from RuntimeConfig
//   - Start / Stop buttons, manual path input
//   - per-pack Install / Uninstall (Phase 2): user downloads weights
//     out-of-band from the canonical URL shown on each pack, then
//     points the installer at the file. The Rust installer
//     SHA-256-verifies (when a canonical hash is pinned) and
//     atomically renames the file into `~/.kcreate/models/`.

import { useCallback, useEffect, useRef, useState } from "react";

import type {
  LlmStatus,
  ModelPack,
  ModelPackCategory,
  ResourceLimits,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface ModelManagerProps {
  onStatus: (msg: string | null) => void;
}

/// Install / uninstall a model pack. Lives on the parent so the
/// PackCard can call into it via prop drilling without each card
/// having to re-resolve `window.kcreate.aiModel`.
interface PackActions {
  busy: boolean;
  install: (pack: ModelPack) => void;
  uninstall: (pack: ModelPack) => void;
}

export function ModelManager({ onStatus }: ModelManagerProps): JSX.Element {
  const [status, setStatus] = useState<LlmStatus | null>(null);
  const [limits, setLimits] = useState<ResourceLimits | null>(null);
  const [modelPath, setModelPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [packs, setPacks] = useState<ModelPack[]>([]);
  /// Synchronous reentry-guard against the small window where the
  /// user double-clicks Install/Uninstall fast enough that React has
  /// not yet committed the `busy=true` state into the DOM. The Rust
  /// installer writes a `.tmp` file and renames atomically, so two
  /// concurrent `installModelPack` calls would race on that
  /// scratch file. The ref short-circuits the second call *before*
  /// any IPC fires, eliminating the race in a way the React batching
  /// scheduler cannot defeat.
  const inFlightPackId = useRef<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [s, l, p] = await Promise.all([
        window.kcreate.llm.status(),
        window.kcreate.runtime.resourceLimits(),
        window.kcreate.aiModel.listModelPacks(),
      ]);
      setStatus(s);
      setLimits(l);
      setPacks(p);
    } catch (e) {
      onStatus(`model status: ${errMsg(e)}`);
    }
  }, [onStatus]);

  useEffect(() => {
    void refresh();
    const id = window.setInterval(() => {
      void refresh();
    }, 2000);
    return () => window.clearInterval(id);
  }, [refresh]);

  const handleStart = useCallback(async () => {
    if (!modelPath.trim()) {
      onStatus("Pick a GGUF model path first.");
      return;
    }
    setBusy(true);
    onStatus("LLM: starting sidecar…");
    try {
      const port = await window.kcreate.llm.start(modelPath.trim());
      onStatus(`LLM: ready on 127.0.0.1:${port}.`);
      await refresh();
    } catch (e) {
      onStatus(`LLM start failed: ${errMsg(e)}`);
    } finally {
      setBusy(false);
    }
  }, [modelPath, onStatus, refresh]);

  const installPack = useCallback(
    async (pack: ModelPack) => {
      if (pack.kind === "built_in") return;
      // Synchronous reentry-guard — see `inFlightPackId` docstring.
      if (inFlightPackId.current !== null) {
        onStatus(
          `${pack.name}: another install/uninstall is already in flight; ignoring duplicate click.`,
        );
        return;
      }
      inFlightPackId.current = pack.id;
      setBusy(true);
      onStatus(`${pack.name}: pick the downloaded weights file…`);
      try {
        const source = await window.kcreate.aiModel.pickModelFile();
        if (!source) {
          onStatus(`${pack.name}: install cancelled.`);
          return;
        }
        onStatus(`${pack.name}: verifying SHA-256…`);
        const report = await window.kcreate.aiModel.installModelPack(
          pack.id,
          source,
        );
        if (report.verified) {
          onStatus(
            `${pack.name}: installed (verified, ${formatBytes(report.sizeBytes)}).`,
          );
        } else {
          onStatus(
            `${pack.name}: installed (UNVERIFIED — actual sha256 ${report.actualSha256.slice(0, 12)}…; registry has no pinned hash yet).`,
          );
        }
        await refresh();
      } catch (e) {
        onStatus(`${pack.name}: install failed: ${errMsg(e)}`);
      } finally {
        inFlightPackId.current = null;
        setBusy(false);
      }
    },
    [onStatus, refresh],
  );

  const uninstallPack = useCallback(
    async (pack: ModelPack) => {
      if (pack.kind === "built_in") return;
      if (inFlightPackId.current !== null) {
        onStatus(
          `${pack.name}: another install/uninstall is already in flight; ignoring duplicate click.`,
        );
        return;
      }
      inFlightPackId.current = pack.id;
      setBusy(true);
      onStatus(`${pack.name}: removing…`);
      try {
        await window.kcreate.aiModel.uninstallModelPack(pack.id);
        onStatus(`${pack.name}: uninstalled.`);
        await refresh();
      } catch (e) {
        onStatus(`${pack.name}: uninstall failed: ${errMsg(e)}`);
      } finally {
        inFlightPackId.current = null;
        setBusy(false);
      }
    },
    [onStatus, refresh],
  );

  const handleStop = useCallback(async () => {
    setBusy(true);
    onStatus("LLM: stopping sidecar…");
    try {
      await window.kcreate.llm.stop();
      onStatus("LLM: stopped.");
      await refresh();
    } catch (e) {
      onStatus(`LLM stop failed: ${errMsg(e)}`);
    } finally {
      setBusy(false);
    }
  }, [onStatus, refresh]);

  const ready = status?.state === "ready";
  const starting = status?.state === "starting";

  return (
    <section style={containerStyle}>
      <header style={headerStyle}>
        <strong>Model Manager</strong>
        <span style={badgeStyle(status?.state ?? "stopped")}>
          {status?.state ?? "stopped"}
        </span>
      </header>

      <dl style={kvListStyle}>
        <KV label="Model">
          <code style={monoStyle}>{status?.model_name ?? "—"}</code>
        </KV>
        <KV label="Port">
          {status?.port ? (
            <code style={monoStyle}>127.0.0.1:{status.port}</code>
          ) : (
            "—"
          )}
        </KV>
        <KV label="Context">
          {status?.context_size ? `${status.context_size} tokens` : "—"}
        </KV>
        <KV label="Device tier">{limits?.deviceTier ?? "—"}</KV>
        <KV label="Max model size">
          {limits ? `${limits.effectiveMaxModelMb} MB` : "—"}
        </KV>
        <KV label="GPU rendering">
          {limits ? (limits.gpuRenderingAllowed ? "allowed" : "disabled") : "—"}
        </KV>
      </dl>

      {status?.state === "error" && status.error ? (
        <p style={errorStyle}>{status.error}</p>
      ) : null}

      <label style={labelStyle}>
        GGUF model path
        <input
          type="text"
          value={modelPath}
          onChange={(e) => setModelPath(e.target.value)}
          placeholder="/Users/you/.kcreate/models/qwen-1.7b.gguf"
          style={inputStyle}
        />
      </label>

      <div style={buttonRowStyle}>
        <button
          type="button"
          onClick={() => {
            void handleStart();
          }}
          disabled={busy || starting || ready || modelPath.trim().length === 0}
          style={primaryBtn(busy || starting || ready || modelPath.trim().length === 0)}
        >
          {starting ? "Starting…" : "Start"}
        </button>
        <button
          type="button"
          onClick={() => {
            void handleStop();
          }}
          disabled={!ready || busy}
          style={secondaryBtn(!ready || busy)}
        >
          Stop
        </button>
      </div>

      <p style={noteStyle}>
        Models run fully offline on this machine. Pick a GGUF file —
        Phase 1 does not bundle a download catalog. The sidecar binds
        to <code style={monoStyle}>127.0.0.1</code> only.
      </p>

      <ModelPacksSection
        packs={packs}
        limits={limits}
        actions={{
          busy,
          install: (pack) => {
            void installPack(pack);
          },
          uninstall: (pack) => {
            void uninstallPack(pack);
          },
        }}
      />
    </section>
  );
}

/// Apply the Phase 4 tier gates to the visible pack list.
///
/// - Generation packs are **hard-gated** on
///   `imageGenerationAllowed`: when the tier+GPU combination
///   forbids it, those packs are filtered out completely (PROPOSAL
///   §7 "hard gate, not a soft one").
/// - Vision packs that exceed `visionModelMaxMb` are still shown
///   but their install action is disabled — the user sees what's
///   available on a beefier tier without being able to install a
///   pack that would never load. We compare in **binary MB**
///   (1024 × 1024) to match the Rust-side cap unit
///   (`crates/kcreate_bridge/src/phase4.rs::vision_listable_packs`)
///   — using decimal MB here would diverge by ~2.4% and could
///   produce edge-case disagreements at tier boundaries.
/// - MLX-suffixed packs are filtered out on non-Apple-Silicon
///   platforms by inspecting `limits.platform` (the `Debug` form of
///   the host `Platform` enum). Earlier code looked at
///   `limits.deviceTier`, but that string only encodes the
///   performance class (`Tier0`/`Tier1`/…) and never contains
///   platform info — so every MLX pack was incorrectly hidden on
///   Apple Silicon too.
const BINARY_MB = 1024 * 1024;
function filterPacksForTier(
  packs: ModelPack[],
  limits: ResourceLimits | null,
): { visible: ModelPack[]; disabledIds: Set<string> } {
  if (!limits) return { visible: packs, disabledIds: new Set() };
  const isAppleSilicon = limits.platform
    .toLowerCase()
    .includes("applesilicon");
  const disabled = new Set<string>();
  const visible = packs.filter((p) => {
    if (p.category === "generation" && !limits.imageGenerationAllowed) {
      return false;
    }
    if (p.id.endsWith("_mlx") && !isAppleSilicon) {
      return false;
    }
    if (p.category === "vision") {
      const sizeMb = p.sizeBytes / BINARY_MB;
      if (sizeMb > limits.visionModelMaxMb) {
        disabled.add(p.id);
      }
    }
    return true;
  });
  return { visible, disabledIds: disabled };
}

function ModelPacksSection({
  packs,
  limits,
  actions,
}: {
  packs: ModelPack[];
  limits: ResourceLimits | null;
  actions: PackActions;
}): JSX.Element {
  if (packs.length === 0) {
    return (
      <p style={noteStyle}>Loading model packs…</p>
    );
  }
  const { visible, disabledIds } = filterPacksForTier(packs, limits);
  const installed = visible.filter((p) => p.installed);
  const available = visible.filter((p) => !p.installed);
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
      <h4 style={{ margin: 0, fontSize: 12, fontWeight: 600 }}>Model packs</h4>
      {installed.length > 0 ? (
        <PackGroup
          label="Installed"
          packs={installed}
          actions={actions}
          disabledIds={disabledIds}
        />
      ) : null}
      {available.length > 0 ? (
        <PackGroup
          label="Available"
          packs={available}
          actions={actions}
          disabledIds={disabledIds}
        />
      ) : null}
    </div>
  );
}

function PackGroup({
  label,
  packs,
  actions,
  disabledIds,
}: {
  label: string;
  packs: ModelPack[];
  actions: PackActions;
  disabledIds: Set<string>;
}): JSX.Element {
  return (
    <section style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      <span style={{ fontSize: 10, color: colors.textMuted, fontWeight: 600, textTransform: "uppercase", letterSpacing: 0.4 }}>{label}</span>
      {packs.map((p) => (
        <PackCard
          key={p.id}
          pack={p}
          actions={actions}
          tierBlocked={disabledIds.has(p.id)}
        />
      ))}
    </section>
  );
}

function PackCard({
  pack,
  actions,
  tierBlocked,
}: {
  pack: ModelPack;
  actions: PackActions;
  tierBlocked: boolean;
}): JSX.Element {
  const optional = pack.kind !== "built_in";
  const installBlocked =
    actions.busy || pack.downloadUrl === "" || tierBlocked;
  return (
    <article
      style={{
        padding: spacing.sm,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.card,
        background: colors.bg,
        display: "flex",
        flexDirection: "column",
        gap: 4,
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: spacing.sm }}>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
            <strong style={{ fontSize: 12 }}>{pack.name}</strong>
            <CategoryPill category={pack.category} />
          </div>
          <div
            style={{
              fontSize: 10,
              color: colors.textMuted,
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
          >
            {pack.capabilities.join(", ") || "—"}
          </div>
        </div>
        <span style={{ fontSize: 10, color: colors.textMuted }}>
          {pack.kind === "built_in"
            ? "built-in"
            : formatBytes(pack.sizeBytes)}
        </span>
        {optional ? (
          pack.installed ? (
            <button
              type="button"
              onClick={() => actions.uninstall(pack)}
              disabled={actions.busy}
              style={packSecondaryBtnStyle(actions.busy)}
            >
              Uninstall
            </button>
          ) : (
            <button
              type="button"
              onClick={() => actions.install(pack)}
              disabled={installBlocked}
              style={packPrimaryBtnStyle(installBlocked)}
              title={
                tierBlocked
                  ? "Exceeds this machine's vision-model size cap. " +
                    "Upgrade hardware or pick a smaller pack."
                  : undefined
              }
            >
              {tierBlocked ? "Tier locked" : "Install…"}
            </button>
          )
        ) : null}
      </div>
      {optional && pack.downloadUrl && !pack.installed ? (
        <a
          href={pack.downloadUrl}
          target="_blank"
          rel="noreferrer"
          style={{
            fontSize: 10,
            color: colors.accent,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
          title={pack.downloadUrl}
        >
          Download weights ↗
        </a>
      ) : null}
    </article>
  );
}

/// Format a byte count for display. `0` would otherwise render as
/// "0 MB" which is misleading for built-in packs, so the caller is
/// expected to avoid passing 0 here.
function formatBytes(bytes: number): string {
  if (bytes >= 1_000_000_000) return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
  if (bytes >= 1_000_000) return `${(bytes / 1_000_000).toFixed(0)} MB`;
  if (bytes >= 1_000) return `${(bytes / 1_000).toFixed(0)} KB`;
  return `${bytes} B`;
}

function packPrimaryBtnStyle(disabled: boolean): React.CSSProperties {
  return {
    padding: "3px 8px",
    background: disabled ? colors.bgSoft : colors.accent,
    color: disabled ? colors.textMuted : "white",
    border: "none",
    borderRadius: radius.pill,
    cursor: disabled ? "default" : "pointer",
    fontSize: 10,
    fontWeight: 600,
  };
}

function packSecondaryBtnStyle(disabled: boolean): React.CSSProperties {
  return {
    padding: "3px 8px",
    background: "transparent",
    color: disabled ? colors.textMuted : colors.text,
    border: `1px solid ${colors.border}`,
    borderRadius: radius.pill,
    cursor: disabled ? "default" : "pointer",
    fontSize: 10,
    fontWeight: 600,
  };
}

function CategoryPill({
  category,
}: {
  category: ModelPackCategory;
}): JSX.Element {
  const labels: Record<ModelPackCategory, string> = {
    core: "core",
    image_pro: "image",
    design_pro: "design",
    vision: "vision",
    generation: "gen",
  };
  return (
    <span
      style={{
        padding: "1px 6px",
        background: colors.bgSoft,
        color: colors.accent,
        borderRadius: radius.pill,
        fontSize: 9,
        fontWeight: 600,
        textTransform: "uppercase",
      }}
    >
      {labels[category]}
    </span>
  );
}

function KV({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}): JSX.Element {
  return (
    <>
      <dt style={kvLabelStyle}>{label}</dt>
      <dd style={kvValueStyle}>{children}</dd>
    </>
  );
}

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

const containerStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.sm,
  marginTop: spacing.md,
  padding: spacing.sm,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
  background: colors.bgSoft,
};

const headerStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  fontSize: 13,
  color: colors.text,
};

function badgeStyle(state: string): React.CSSProperties {
  const tones: Record<string, string> = {
    ready: "#0F766E",
    starting: colors.accent,
    error: "#DC2626",
    stopped: colors.textMuted,
  };
  return {
    fontSize: 10,
    fontWeight: 700,
    textTransform: "uppercase",
    letterSpacing: 0.4,
    color: tones[state] ?? colors.textMuted,
  };
}

const kvListStyle: React.CSSProperties = {
  margin: 0,
  display: "grid",
  gridTemplateColumns: "auto 1fr",
  gap: "2px 8px",
  fontSize: 11,
};

const kvLabelStyle: React.CSSProperties = {
  color: colors.textMuted,
  fontWeight: 500,
  margin: 0,
};

const kvValueStyle: React.CSSProperties = {
  color: colors.text,
  margin: 0,
};

const labelStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.xs,
  fontSize: 11,
  color: colors.textMuted,
};

const inputStyle: React.CSSProperties = {
  padding: `${spacing.xs}px ${spacing.sm}px`,
  fontSize: 12,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.pill,
  background: colors.bg,
  color: colors.text,
};

const buttonRowStyle: React.CSSProperties = {
  display: "flex",
  gap: spacing.xs,
};

function primaryBtn(disabled: boolean): React.CSSProperties {
  return {
    flex: 1,
    padding: "6px 12px",
    fontSize: 12,
    fontWeight: 600,
    background: disabled ? colors.bgSoft : colors.accent,
    color: disabled ? colors.textMuted : colors.textInverse,
    border: `1px solid ${disabled ? colors.border : colors.accent}`,
    borderRadius: radius.pill,
    cursor: disabled ? "not-allowed" : "pointer",
  };
}

function secondaryBtn(disabled: boolean): React.CSSProperties {
  return {
    padding: "6px 12px",
    fontSize: 12,
    fontWeight: 500,
    background: colors.bg,
    color: colors.text,
    border: `1px solid ${colors.border}`,
    borderRadius: radius.pill,
    cursor: disabled ? "not-allowed" : "pointer",
    opacity: disabled ? 0.5 : 1,
  };
}

const errorStyle: React.CSSProperties = {
  margin: 0,
  padding: `${spacing.xs}px ${spacing.sm}px`,
  fontSize: 11,
  color: "#DC2626",
  background: "rgba(220,38,38,0.08)",
  border: "1px solid #DC2626",
  borderRadius: radius.card / 2,
};

const noteStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 10,
  color: colors.textMuted,
  lineHeight: 1.5,
};

const monoStyle: React.CSSProperties = {
  fontFamily:
    'ui-monospace, SFMono-Regular, Menlo, "Roboto Mono", monospace',
  fontSize: 10,
};

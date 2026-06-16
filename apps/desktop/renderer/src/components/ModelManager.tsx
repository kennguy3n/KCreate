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
  ModelDownloadProgress,
  ModelPack,
  ModelPackCategory,
  ResourceLimits,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface ModelManagerProps {
  onStatus: (msg: string | null) => void;
}

/// Install / uninstall / download a model pack. Lives on the parent
/// so the PackCard can call into it via prop drilling without each
/// card having to re-resolve `window.kcreate.aiModel`.
interface PackActions {
  busy: boolean;
  install: (pack: ModelPack) => void;
  uninstall: (pack: ModelPack) => void;
  /// Start an in-app, progress-reporting download of the pack's
  /// weights (the main process resolves the URL + verifies the
  /// checksum). Only one download runs at a time.
  download: (pack: ModelPack) => void;
  /// Abort the in-flight download, if any.
  cancelDownload: () => void;
  /// Id of the pack currently downloading, or `null`. The card whose
  /// `pack.id` matches renders the progress bar + Cancel button.
  downloadingPackId: string | null;
  /// Latest progress event for the in-flight download, or `null`
  /// before the first event lands.
  progress: ModelDownloadProgress | null;
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
  /// Id of the pack whose weights are downloading in-app, or `null`.
  /// Set when the user clicks Download; cleared when the download
  /// settles (done / error / cancelled). The PackCard whose id
  /// matches renders the progress bar.
  const [downloadingPackId, setDownloadingPackId] = useState<string | null>(
    null,
  );
  /// Latest progress event for the in-flight download. The main
  /// process is single-flight per channel, so a single slot is
  /// sufficient — events for the resolving / error phases carry an
  /// empty packId, which is why the card-to-progress association is
  /// keyed off `downloadingPackId` (the pack the user clicked) rather
  /// than the event's own `packId`.
  const [downloadProgress, setDownloadProgress] =
    useState<ModelDownloadProgress | null>(null);

  // Subscribe once to main-process download progress. The unsubscribe
  // handle is synchronous (preload returns a plain function), so the
  // effect cleanup tears the listener down on unmount to avoid leaking
  // IPC listeners across the panel's mount lifecycle.
  useEffect(() => {
    const unsubscribe = window.kcreate.aiModel.onModelDownloadProgress(
      (progress) => {
        setDownloadProgress(progress);
      },
    );
    return unsubscribe;
  }, []);

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

  const downloadPack = useCallback(
    async (pack: ModelPack) => {
      if (pack.kind === "built_in") return;
      if (pack.downloadUrl === "") {
        onStatus(`${pack.name}: no download URL pinned in the registry.`);
        return;
      }
      // Same synchronous reentry-guard as install — the main process
      // is single-flight, but guarding here avoids a second IPC round
      // trip (which would cancel the first download) on a fast
      // double-click.
      if (inFlightPackId.current !== null) {
        onStatus(
          `${pack.name}: another model action is already in flight; ignoring duplicate click.`,
        );
        return;
      }
      inFlightPackId.current = pack.id;
      setBusy(true);
      setDownloadingPackId(pack.id);
      setDownloadProgress(null);
      onStatus(`${pack.name}: downloading weights…`);
      try {
        const report = await window.kcreate.aiModel.downloadModelPack(pack.id);
        if (report.verified) {
          onStatus(
            `${pack.name}: downloaded & installed (verified, ${formatBytes(report.sizeBytes)}).`,
          );
        } else {
          onStatus(
            `${pack.name}: downloaded & installed (UNVERIFIED — actual sha256 ${report.actualSha256.slice(0, 12)}…; registry has no pinned hash yet).`,
          );
        }
        await refresh();
      } catch (e) {
        if (errMsg(e) === "cancelled") {
          onStatus(`${pack.name}: download cancelled.`);
        } else {
          onStatus(`${pack.name}: download failed: ${errMsg(e)}`);
        }
      } finally {
        inFlightPackId.current = null;
        setBusy(false);
        setDownloadingPackId(null);
        setDownloadProgress(null);
      }
    },
    [onStatus, refresh],
  );

  const cancelDownload = useCallback(() => {
    void window.kcreate.aiModel.cancelModelDownload();
  }, []);

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
          download: (pack) => {
            void downloadPack(pack);
          },
          cancelDownload,
          downloadingPackId,
          progress: downloadProgress,
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
/// - Phase 12 removed every `_mlx`-suffixed pack from the registry
///   (the MLX sidecar is gone), so the historical
///   "hide MLX packs on non-Apple-Silicon" branch is no longer
///   needed. Every pack the bridge surfaces here is GGUF and runs
///   on the same llama-server / sd-server stack regardless of
///   host platform.
const BINARY_MB = 1024 * 1024;
function filterPacksForTier(
  packs: ModelPack[],
  limits: ResourceLimits | null,
): { visible: ModelPack[]; disabledIds: Set<string> } {
  if (!limits) return { visible: packs, disabledIds: new Set() };
  const disabled = new Set<string>();
  const visible = packs.filter((p) => {
    if (p.category === "generation" && !limits.imageGenerationAllowed) {
      return false;
    }
    if (p.category === "vision") {
      // Use `Math.floor` so this matches the Rust side's `u64`
      // integer division exactly (see
      // `crates/kcreate_bridge/src/phase4.rs::vision_listable_packs`
      // — `p.size_bytes / (1024 * 1024)`). JS `/` is float64
      // division, so without `floor` a pack whose byte count
      // floats to e.g. 500.0005 MB would be disabled here but
      // accepted by the Rust filter — a wire-format-lockstep
      // (AGENTS.md §4) divergence at tier boundaries. Today's
      // packs are well below their caps so the float vs int
      // delta is unreachable, but we pin the contract now.
      const sizeMb = Math.floor(p.sizeBytes / BINARY_MB);
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
  // Disk usage attributable to optional packs the user installed.
  // Built-in packs ship `sizeBytes === 0` (they live in the app
  // bundle, not `models_dir`), so they contribute nothing and the
  // total honestly reflects what the user downloaded onto disk.
  const installedBytes = installed.reduce((sum, p) => sum + p.sizeBytes, 0);
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
      <div
        style={{
          display: "flex",
          alignItems: "baseline",
          justifyContent: "space-between",
        }}
      >
        <h4 style={{ margin: 0, fontSize: 12, fontWeight: 600 }}>Model packs</h4>
        <span style={{ fontSize: 10, color: colors.textMuted }}>
          {installed.length > 0
            ? `${installed.length} installed · ${formatBytes(installedBytes)} on disk`
            : "none installed"}
        </span>
      </div>
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
  const isDownloading = actions.downloadingPackId === pack.id;
  // Manual "pick a file you already have" path. Disabled while any
  // model action is busy or the pack is tier-locked; unlike Download
  // it stays enabled when no URL is pinned (the whole point of the
  // manual path is to install a pack the registry can't fetch).
  const installBlocked = actions.busy || tierBlocked;
  // In-app download path. Additionally requires a pinned URL.
  const downloadBlocked =
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
          ) : isDownloading ? (
            <button
              type="button"
              onClick={() => actions.cancelDownload()}
              style={packSecondaryBtnStyle(false)}
            >
              Cancel
            </button>
          ) : (
            <div style={{ display: "flex", gap: 4 }}>
              <button
                type="button"
                onClick={() => actions.download(pack)}
                disabled={downloadBlocked}
                style={packPrimaryBtnStyle(downloadBlocked)}
                title={
                  tierBlocked
                    ? "Exceeds this machine's vision-model size cap. " +
                      "Upgrade hardware or pick a smaller pack."
                    : pack.downloadUrl === ""
                      ? "No download URL pinned in the registry — use " +
                        "‘File…’ to install a copy you already have."
                      : "Download & verify the weights in-app."
                }
              >
                {tierBlocked ? "Tier locked" : "Download"}
              </button>
              <button
                type="button"
                onClick={() => actions.install(pack)}
                disabled={installBlocked}
                style={packSecondaryBtnStyle(installBlocked)}
                title="Install from a weights file you already downloaded."
              >
                File…
              </button>
            </div>
          )
        ) : null}
      </div>
      {isDownloading ? (
        <DownloadProgressBar progress={actions.progress} />
      ) : null}
      {optional && pack.downloadUrl && !pack.installed && !isDownloading ? (
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
          Download weights manually ↗
        </a>
      ) : null}
    </article>
  );
}

/// Renders the live progress of an in-app model download. Shows a
/// determinate bar once the server reports a `Content-Length`, an
/// indeterminate "shimmer-free" full-width track until then, and an
/// accessible phase label (`Connecting…` / `45% · 1.1 GB / 2.4 GB` /
/// `Verifying…` / `Installing…`). The bar is driven entirely by the
/// main-process progress events — the renderer never touches the
/// network.
function DownloadProgressBar({
  progress,
}: {
  progress: ModelDownloadProgress | null;
}): JSX.Element {
  const phase = progress?.phase ?? "resolving";
  const received = progress?.receivedBytes ?? 0;
  const total = progress?.totalBytes ?? null;
  const pct =
    total && total > 0
      ? Math.min(100, Math.round((received / total) * 100))
      : null;
  const label = downloadPhaseLabel(phase, received, total, pct);
  // `aria-busy` while the download is actively running so assistive
  // tech announces the in-progress state; the role+valuenow expose the
  // determinate percentage when known.
  const active =
    phase !== "done" && phase !== "error" && phase !== "cancelled";
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 2 }}>
      <div
        role="progressbar"
        aria-busy={active}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pct ?? undefined}
        aria-label={`Download ${label}`}
        style={progressTrackStyle}
      >
        <div
          style={{
            ...progressFillStyle,
            width: pct === null ? "100%" : `${pct}%`,
            opacity: pct === null ? 0.4 : 1,
          }}
        />
      </div>
      <span style={{ fontSize: 9, color: colors.textMuted }}>{label}</span>
    </div>
  );
}

/// Build the human-readable status line for a download phase. Kept
/// pure (no React) so the ModelManager test can assert the exact
/// copy without rendering.
function downloadPhaseLabel(
  phase: ModelDownloadProgress["phase"],
  received: number,
  total: number | null,
  pct: number | null,
): string {
  switch (phase) {
    case "resolving":
      return "Preparing…";
    case "connecting":
      return "Connecting…";
    case "downloading":
      return pct === null
        ? `Downloading… ${formatBytes(received)}`
        : `${pct}% · ${formatBytes(received)} / ${formatBytes(total ?? received)}`;
    case "verifying":
      return "Verifying checksum…";
    case "installing":
      return "Installing…";
    case "done":
      return "Done.";
    case "cancelled":
      return "Cancelled.";
    case "error":
      return "Failed.";
    default:
      return "";
  }
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

const progressTrackStyle: React.CSSProperties = {
  width: "100%",
  height: 4,
  borderRadius: radius.pill,
  background: colors.bgSoft,
  overflow: "hidden",
};

const progressFillStyle: React.CSSProperties = {
  height: "100%",
  background: colors.accent,
  borderRadius: radius.pill,
  transition: "width 120ms linear",
};

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
    ready: colors.success,
    starting: colors.accent,
    error: colors.danger,
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
  color: colors.danger,
  background: colors.dangerBgSoft,
  border: `1px solid ${colors.danger}`,
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

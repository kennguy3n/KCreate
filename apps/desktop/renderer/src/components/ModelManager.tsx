// ModelManager — local AI model lifecycle UI.
//
// Phase 1 surfaces:
//   - currently-loaded model (name, status, listening port, device tier)
//   - effective max-model-size budget from RuntimeConfig
//   - Start / Stop buttons, manual path input
//
// Download from a curated catalog is Phase 2; the user manually
// points to a GGUF file for now.

import { useCallback, useEffect, useState } from "react";

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

export function ModelManager({ onStatus }: ModelManagerProps): JSX.Element {
  const [status, setStatus] = useState<LlmStatus | null>(null);
  const [limits, setLimits] = useState<ResourceLimits | null>(null);
  const [modelPath, setModelPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [packs, setPacks] = useState<ModelPack[]>([]);

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

      <ModelPacksSection packs={packs} />
    </section>
  );
}

function ModelPacksSection({
  packs,
}: {
  packs: ModelPack[];
}): JSX.Element {
  if (packs.length === 0) {
    return (
      <p style={noteStyle}>Loading model packs…</p>
    );
  }
  const installed = packs.filter((p) => p.installed);
  const available = packs.filter((p) => !p.installed);
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
      <h4 style={{ margin: 0, fontSize: 12, fontWeight: 600 }}>Model packs</h4>
      {installed.length > 0 ? (
        <PackGroup label="Installed" packs={installed} />
      ) : null}
      {available.length > 0 ? (
        <PackGroup label="Available" packs={available} />
      ) : null}
    </div>
  );
}

function PackGroup({
  label,
  packs,
}: {
  label: string;
  packs: ModelPack[];
}): JSX.Element {
  return (
    <section style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      <span style={{ fontSize: 10, color: colors.textMuted, fontWeight: 600, textTransform: "uppercase", letterSpacing: 0.4 }}>{label}</span>
      {packs.map((p) => (
        <PackCard key={p.id} pack={p} />
      ))}
    </section>
  );
}

function PackCard({ pack }: { pack: ModelPack }): JSX.Element {
  return (
    <article
      style={{
        padding: spacing.sm,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.card,
        background: colors.bg,
        display: "flex",
        alignItems: "center",
        gap: spacing.sm,
      }}
    >
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
          : `${(pack.size_bytes / 1024 / 1024).toFixed(0)} MB`}
      </span>
    </article>
  );
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

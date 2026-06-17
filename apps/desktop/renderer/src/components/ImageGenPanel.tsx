// ImageGenPanel — Tier 2+ FLUX image generation.
//
// HARD gate: this panel renders `null` when
// `window.kcreate.imageGen.allowed()` returns false. There's no
// "feature locked" splash, no "upgrade" CTA — the UI element
// simply doesn't exist. This matches the PROPOSAL.md hard-gate
// requirement that Tier 0/1 users see no image-gen affordances at
// all, not even ghosted ones.
//
// Wire shape:
//   1. Probe `imageGen.allowed()` once at mount. If false, render
//      nothing. Don't subscribe to anything else.
//   2. If allowed, query `imageGen.status()` and let the user start
//      the sidecar via `imageGen.start(packId)`.
//   3. Prompt → Generate → Preview → Apply (insert as raster).

import { useEffect, useState } from "react";

import type {
  ImageGenStatus,
  ImageGenEngine,
  GeneratedImage,
  ModelPack,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

// Human-readable label for the engine actually driving inference.
// `sd_cpp` is the universal stable-diffusion.cpp sidecar (SD 1.5 /
// FLUX Klein); the two Bonsai engines are hardware-locked runners.
function engineLabel(engine: ImageGenEngine | null): string {
  switch (engine) {
    case "bonsai_mlx":
      return "bonsai-mlx";
    case "bonsai_gemlite":
      return "bonsai-gemlite";
    case "sd_cpp":
      return "sd-server";
    default:
      return "stopped";
  }
}

// Bonsai image packs are strictly opt-in: they're only ever the active
// model when the user explicitly picks them, never via an automatic
// default. Centralised so the pack-id prefix lives in one place.
function isBonsaiPackId(id: string): boolean {
  return id.startsWith("image_gen_bonsai_");
}

// Choose the pack the generation selector defaults to. The
// device-recommended pack wins whenever it's present in the advertised
// list; otherwise we fall through installed → advertised packs, always
// skipping Bonsai so it stays strictly opt-in and never becomes an
// automatic default (even if `recommendedPack()` ever returns "").
export function pickDefaultGenerationPack(
  gens: ModelPack[],
  recommended: string,
): string {
  if (recommended.length > 0 && gens.some((p) => p.id === recommended)) {
    return recommended;
  }
  return (
    gens.find((p) => p.installed && !isBonsaiPackId(p.id))?.id ??
    gens.find((p) => !isBonsaiPackId(p.id))?.id ??
    gens[0]?.id ??
    ""
  );
}

interface ImageGenPanelProps {
  onStatus: (msg: string | null) => void;
  onApplied: () => void;
}

type Phase = "idle" | "starting" | "ready" | "running" | "preview" | "error";

const SIZES: Array<{ label: string; w: number; h: number }> = [
  { label: "Square 1024", w: 1024, h: 1024 },
  { label: "Portrait 768x1024", w: 768, h: 1024 },
  { label: "Landscape 1024x768", w: 1024, h: 768 },
  { label: "Banner 1280x512", w: 1280, h: 512 },
];

export function ImageGenPanel({
  onStatus,
  onApplied,
}: ImageGenPanelProps): JSX.Element | null {
  const [allowed, setAllowed] = useState<boolean | null>(null);
  const [status, setStatus] = useState<ImageGenStatus | null>(null);
  const [recommended, setRecommended] = useState<string>("");
  const [genPacks, setGenPacks] = useState<ModelPack[]>([]);
  const [selectedPack, setSelectedPack] = useState<string>("");
  const [prompt, setPrompt] = useState<string>("");
  const [sizeIndex, setSizeIndex] = useState<number>(0);
  const [steps, setSteps] = useState<number>(20);
  const [seed, setSeed] = useState<string>("");
  const [phase, setPhase] = useState<Phase>("idle");
  const [error, setError] = useState<string | null>(null);
  const [preview, setPreview] = useState<{
    image: GeneratedImage;
    dataUrl: string;
  } | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const ok = await window.kcreate.imageGen.allowed();
        if (cancelled) return;
        setAllowed(ok);
        if (!ok) return;
        const [s, r, packs] = await Promise.all([
          window.kcreate.imageGen.status(),
          window.kcreate.imageGen.recommendedPack(),
          window.kcreate.aiModel.listModelPacks(),
        ]);
        if (cancelled) return;
        const gens = packs.filter((p) => p.category === "generation");
        setStatus(s);
        setRecommended(r);
        setGenPacks(gens);
        // Default the selector to the device-recommended pack (SD 1.5
        // on a Tier 2 GPU box), never auto-selecting a Bonsai pack.
        setSelectedPack(pickDefaultGenerationPack(gens, r));
      } catch {
        if (!cancelled) setAllowed(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  if (allowed === null) {
    // Still probing — render nothing rather than a placeholder so
    // we don't briefly flash UI that will then disappear on Tier
    // 0/1 hardware.
    return null;
  }
  if (allowed === false) {
    return null;
  }

  const ready = status?.state === "ready";
  const packName = (id: string | null): string =>
    (id ? genPacks.find((p) => p.id === id)?.name : undefined) ?? id ?? "";
  const selectedIsBonsai = isBonsaiPackId(selectedPack);
  // The bridge reports the requested vs. actually-loaded pack; when
  // they differ a Bonsai request degraded to SD 1.5 on this host.
  const fellBack =
    ready &&
    status?.requestedPackId != null &&
    status?.activePackId != null &&
    status.requestedPackId !== status.activePackId;

  const refresh = async (): Promise<void> => {
    const s = await window.kcreate.imageGen.status();
    setStatus(s);
  };

  const startSidecar = async (): Promise<void> => {
    if (!selectedPack) return;
    setPhase("starting");
    onStatus("Starting image-gen sidecar…");
    try {
      await window.kcreate.imageGen.start(selectedPack);
      await refresh();
      setPhase("ready");
      onStatus("Image-gen sidecar ready.");
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
      setPhase("error");
      onStatus(`Image-gen start failed: ${msg}`);
    }
  };

  const generate = async (): Promise<void> => {
    if (!ready || prompt.trim().length === 0) return;
    const size: { label: string; w: number; h: number } =
      SIZES[sizeIndex] ?? SIZES[0]!;
    setPhase("running");
    setError(null);
    onStatus(`AI: generating ${size.w}x${size.h} image…`);
    try {
      const parsedSeed = seed.trim().length > 0 ? Number(seed) : null;
      const result = await window.kcreate.imageGen.generate(
        prompt.trim(),
        size.w,
        size.h,
        steps,
        parsedSeed,
      );
      // The Rust side returns PNG bytes as a base64 string. Inline
      // them directly as a `data:` URL for the preview — no extra
      // decode pass needed for display.
      setPreview({
        image: result,
        dataUrl: `data:image/png;base64,${result.pngB64}`,
      });
      setPhase("preview");
      onStatus("AI: generation ready.");
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
      setPhase("error");
      onStatus(`AI generate failed: ${msg}`);
    }
  };

  const apply = async (): Promise<void> => {
    if (!preview) return;
    try {
      // Decode the base64 PNG and hand the bytes to the document
      // bridge. `importImageBytes` re-checks magic bytes and stores
      // the blob with the right MIME — we don't have to know the
      // pixel data here.
      const bytes = base64ToBytes(preview.image.pngB64);
      await window.kcreate.canvas.importImageBytes(null, bytes);
      setPhase("ready");
      setPreview(null);
      setPrompt("");
      onStatus("Generated image inserted.");
      onApplied();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
      setPhase("error");
      onStatus(`Insert failed: ${msg}`);
    }
  };

  return (
    <section style={cardStyle}>
      <div style={cardHeaderStyle}>
        <strong>Image generation</strong>
        <span style={badgeStyle(ready ? "ok" : "warn")}>
          {ready
            ? `${engineLabel(status?.engine ?? null)} :${status?.port ?? "?"}`
            : (status?.state ?? "stopped")}
        </span>
      </div>
      <p style={hintStyle}>
        Local diffusion inference. Runs entirely on this machine — no
        data leaves your device. Tier 2+ GPU recommended.
      </p>
      {!ready ? (
        <>
          {genPacks.length > 0 ? (
            <label style={fieldStyle}>
              <span style={fieldLabelStyle}>Model</span>
              <select
                value={selectedPack}
                onChange={(e) => setSelectedPack(e.target.value)}
                disabled={phase === "starting"}
                style={selectStyle}
              >
                {genPacks.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                    {p.installed ? "" : " — not downloaded"}
                    {p.id === recommended ? " (recommended)" : ""}
                  </option>
                ))}
              </select>
            </label>
          ) : null}
          {selectedIsBonsai ? (
            <p style={hintStyle}>
              Bonsai ternary 4B runs on Apple Silicon (MLX) or a CUDA
              GPU (GemLite). On other hardware KCreate falls back to SD
              1.5 automatically.
            </p>
          ) : null}
          <button
            type="button"
            onClick={() => {
              void startSidecar();
            }}
            disabled={!selectedPack || phase === "starting"}
            style={primaryBtn(!selectedPack || phase === "starting")}
          >
            {phase === "starting"
              ? "Starting…"
              : selectedPack
                ? `Start (${packName(selectedPack)})`
                : "No generation pack installed"}
          </button>
        </>
      ) : null}
      {fellBack ? (
        <div style={statusStripStyle("ok")}>
          Requested {packName(status?.requestedPackId ?? null)} isn&apos;t
          runnable on this device — running{" "}
          {packName(status?.activePackId ?? null)} instead.
        </div>
      ) : null}
      <textarea
        value={prompt}
        onChange={(e) => setPrompt(e.target.value)}
        placeholder="Describe the image you want…"
        rows={3}
        disabled={!ready || phase === "running"}
        style={textareaStyle}
      />
      <div style={{ display: "flex", gap: spacing.xs, flexWrap: "wrap" }}>
        <label style={fieldStyle}>
          <span style={fieldLabelStyle}>Size</span>
          <select
            value={sizeIndex}
            onChange={(e) => setSizeIndex(Number(e.target.value))}
            disabled={!ready || phase === "running"}
            style={selectStyle}
          >
            {SIZES.map((s, i) => (
              <option key={s.label} value={i}>
                {s.label}
              </option>
            ))}
          </select>
        </label>
        <label style={fieldStyle}>
          <span style={fieldLabelStyle}>Steps</span>
          <input
            type="number"
            min={1}
            max={100}
            value={steps}
            onChange={(e) => setSteps(Math.max(1, Math.min(100, Number(e.target.value))))}
            disabled={!ready || phase === "running"}
            style={selectStyle}
          />
        </label>
        <label style={fieldStyle}>
          <span style={fieldLabelStyle}>Seed</span>
          <input
            type="text"
            placeholder="random"
            value={seed}
            onChange={(e) => setSeed(e.target.value.replace(/[^0-9]/g, ""))}
            disabled={!ready || phase === "running"}
            style={selectStyle}
          />
        </label>
      </div>
      <button
        type="button"
        onClick={() => {
          void generate();
        }}
        disabled={!ready || prompt.trim().length === 0 || phase === "running"}
        style={primaryBtn(!ready || prompt.trim().length === 0 || phase === "running")}
      >
        {phase === "running" ? "Generating…" : "Generate"}
      </button>
      {phase === "preview" && preview ? (
        <div style={resultBoxStyle}>
          <img
            src={preview.dataUrl}
            alt="Generated preview"
            style={previewImageStyle}
          />
          <button
            type="button"
            onClick={() => {
              void apply();
            }}
            style={primaryBtn(false)}
          >
            Insert as new layer
          </button>
        </div>
      ) : null}
      {phase === "error" && error ? (
        <div style={statusStripStyle("err")}>{error}</div>
      ) : null}
    </section>
  );
}

function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i += 1) {
    out[i] = bin.charCodeAt(i);
  }
  return out;
}

const cardStyle: React.CSSProperties = {
  background: colors.bgSoft,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
  padding: spacing.md,
  display: "flex",
  flexDirection: "column",
  gap: spacing.sm,
};
const cardHeaderStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
};
const hintStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 11,
  color: colors.textMuted,
  lineHeight: 1.5,
};
function badgeStyle(kind: "ok" | "warn"): React.CSSProperties {
  return {
    fontSize: 10,
    fontWeight: 600,
    padding: "2px 6px",
    borderRadius: radius.pill,
    background:
      kind === "ok" ? "rgba(124,58,237,0.15)" : colors.dangerBgSoft,
    color: kind === "ok" ? colors.accent : colors.danger,
    textTransform: "uppercase",
    letterSpacing: 0.4,
  };
}
function primaryBtn(disabled: boolean): React.CSSProperties {
  return {
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
const textareaStyle: React.CSSProperties = {
  fontSize: 12,
  padding: spacing.xs,
  borderRadius: radius.sm,
  border: `1px solid ${colors.border}`,
  background: colors.bg,
  color: colors.text,
  resize: "vertical",
  fontFamily: "inherit",
};
const fieldStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 2,
  flex: "1 1 80px",
};
const fieldLabelStyle: React.CSSProperties = {
  fontSize: 10,
  color: colors.textMuted,
};
const selectStyle: React.CSSProperties = {
  fontSize: 12,
  padding: spacing.xs,
  borderRadius: radius.sm,
  border: `1px solid ${colors.border}`,
  background: colors.bg,
  color: colors.text,
};
const resultBoxStyle: React.CSSProperties = {
  background: colors.bg,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.md,
  padding: spacing.sm,
  display: "flex",
  flexDirection: "column",
  gap: spacing.sm,
};
const previewImageStyle: React.CSSProperties = {
  width: "100%",
  height: "auto",
  borderRadius: radius.sm,
};
function statusStripStyle(kind: "ok" | "err"): React.CSSProperties {
  return {
    padding: `${spacing.xs}px ${spacing.sm}px`,
    fontSize: 11,
    borderRadius: radius.md,
    background:
      kind === "ok" ? "rgba(124,58,237,0.08)" : colors.dangerBgSoft,
    color: kind === "ok" ? colors.accent : colors.danger,
    border: `1px solid ${kind === "ok" ? colors.accent : colors.danger}`,
  };
}

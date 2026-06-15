// Phase 9 Block B Task 7 — "Start from a brief" modal.
//
// PROPOSAL.md §4.1: "A 'Start from a brief' tile that opens a local
// LLM prompt; the model fills out an artboard preset, palette, and
// starter layers."
//
// The modal hosts two complementary flows:
//
//   * "Themed design" (G3 — Gamma-style): pick Deck vs One-pager and a
//     built-in professional theme, type a brief, hit Generate, and land
//     on a fully populated, themed, multi-page document. This path is
//     deterministic (`kcreate_ai::themed_deck`) so it works with NO
//     local model loaded; when the sidecar is ready the user can opt in
//     to LLM enrichment, which expands the brief into a structured
//     outline and falls back to the deterministic planner on any
//     failure.
//   * "Single artboard" (the original Phase 9 flow): the local LLM fills
//     out one artboard preset + palette + starter layers. Requires the
//     sidecar to be ready.
//
// Both flows materialise a scratch project when the modal is opened
// from the Home page (no workspace mounted yet) before mutating the
// document, mirroring the "Create new" tile in `App.handleOpenEditor`.
// The user can always cancel; nothing is persisted until the bridge
// call completes successfully.

import { useCallback, useEffect, useMemo, useState } from "react";
import type {
  ArtboardPreset,
  BriefApplyResult,
  BriefPlan,
  LlmMessage,
  ThemedDesignApplyResult,
  ThemedDesignFormat,
  ThemedDesignOptions,
  ThemedOnePagerSize,
  ThemeId,
} from "../../../shared/scene";
import { openScratchProject } from "../lib/scratchProject";
import { colors, font, radius, spacing } from "../styles/tokens";

/**
 * Collapse a preset name to a loose-match key: lowercase ASCII
 * alphanumerics only. Mirrors `normalize_preset_name` in
 * `crates/kcreate_bridge/src/phase9.rs` so the renderer's
 * client-side validator agrees with the bridge’s server-side
 * matcher on which strings resolve to the same preset.
 */
function normalizePresetName(name: string): string {
  let out = "";
  for (const ch of name) {
    if ((ch >= "a" && ch <= "z") || (ch >= "0" && ch <= "9")) {
      out += ch;
    } else if (ch >= "A" && ch <= "Z") {
      out += ch.toLowerCase();
    }
  }
  return out;
}

/**
 * Built-in themes offered by the Gamma-style generator. The `id`
 * values are the wire strings the bridge accepts
 * (`kcreate_ai::themed_deck::ThemeId`); the swatches are a small,
 * representative slice of each theme's palette (background, primary,
 * secondary) used purely to preview the look in the picker. The
 * authoritative palette lives in Rust — these never reach the
 * document.
 */
const THEME_OPTIONS: ReadonlyArray<{
  id: ThemeId;
  label: string;
  swatches: readonly [string, string, string];
}> = [
  { id: "midnight", label: "Midnight", swatches: ["#0B1020", "#7C5CFF", "#34D8FF"] },
  { id: "sunrise", label: "Sunrise", swatches: ["#FBF6EF", "#E2603B", "#F2A65A"] },
  { id: "forest", label: "Forest", swatches: ["#FFFFFF", "#1E8E5A", "#0F6E6E"] },
  { id: "ember", label: "Ember", swatches: ["#121212", "#FF8A3D", "#FFC857"] },
  { id: "slate", label: "Slate", swatches: ["#EEF2F7", "#2563EB", "#0EA5E9"] },
];

interface BriefModalProps {
  open: boolean;
  onClose: () => void;
  /**
   * Whether the local LLM sidecar is ready. Gates the "Single
   * artboard" plan flow (which has no deterministic fallback) and the
   * optional "expand with AI" toggle on the themed flow. The themed
   * generator itself works regardless.
   */
  llmReady?: boolean;
  /**
   * Fired with the bridge result so the shell can navigate into the
   * editor. The shell only uses it to confirm an apply happened (it
   * re-reads project info itself), so either result shape is accepted.
   */
  onApplied: (result: BriefApplyResult | ThemedDesignApplyResult) => void;
}

/**
 * Build the LLM system prompt with the LIVE list of artboard
 * preset display names taken from `window.kcreate.artboard.presets()`.
 *
 * Why dynamic: the previous revision hardcoded a synthetic camelCase
 * list (`"instagramPost"`, `"a4"`, …) that did not match any name
 * `standard_presets()` actually returns on the Rust side
 * (`"Instagram Post"`, `"A4"`, …). The exact-match in
 * `brief_to_project` would have failed for every LLM-produced plan,
 * silently breaking the entire brief→project flow end-to-end. The
 * Rust matcher now normalizes both sides as a defense-in-depth
 * layer, but the prompt is the *primary* source of truth for what
 * the LLM emits, so it must enumerate the real names instead of an
 * out-of-band synthetic set.
 */
function buildSystemPrompt(presets: readonly ArtboardPreset[]): string {
  const names = presets.map((p) => `"${p.name}"`).join(", ");
  return `You are an expert graphic-design planner. The
user will describe a design they want and you will reply with STRICT
JSON in this shape — no Markdown, no commentary, no extra fields:

{
  "artboardPreset": string,      // exactly one of: ${names}
  "palette": string[],           // 3-6 hex colors like "#1F2937"
  "starterLayers": [
    { "name": string, "kind": "text"|"shape"|"image"|"group",
      "suggestedContent": string|null }
  ]
}

Pick the artboardPreset that best matches the user's brief. Pick a
palette that fits the brand mood. Suggest 3-8 starter layers (title,
subtitle, hero shape, etc.). Reply ONLY with the JSON object.`;
}

function parseBriefPlan(
  raw: string,
  presetKeys: ReadonlySet<string>,
): BriefPlan {
  // The LLM occasionally wraps replies in ``` fences even when told
  // not to. Strip them defensively before parsing.
  const trimmed = raw
    .trim()
    .replace(/^```(?:json)?\s*/u, "")
    .replace(/```$/u, "")
    .trim();
  const parsed = JSON.parse(trimmed) as unknown;
  if (typeof parsed !== "object" || parsed === null) {
    throw new Error("LLM reply is not a JSON object");
  }
  const obj = parsed as Record<string, unknown>;
  const artboardPreset = obj["artboardPreset"];
  if (
    typeof artboardPreset !== "string" ||
    !presetKeys.has(normalizePresetName(artboardPreset))
  ) {
    throw new Error(`unknown artboard preset: ${String(artboardPreset)}`);
  }
  const palette = obj["palette"];
  if (!Array.isArray(palette) || palette.some((c) => typeof c !== "string")) {
    throw new Error("palette must be an array of hex strings");
  }
  const layers = obj["starterLayers"];
  if (!Array.isArray(layers)) {
    throw new Error("starterLayers must be an array");
  }
  const starterLayers: BriefPlan["starterLayers"] = layers.map((entry, i) => {
    if (typeof entry !== "object" || entry === null) {
      throw new Error(`starter layer ${i} is not an object`);
    }
    const obj = entry as Record<string, unknown>;
    const name = obj["name"];
    const kind = obj["kind"];
    const suggestedContent = obj["suggestedContent"];
    if (typeof name !== "string" || name.length === 0) {
      throw new Error(`starter layer ${i} has no name`);
    }
    if (
      kind !== "text" &&
      kind !== "shape" &&
      kind !== "image" &&
      kind !== "group"
    ) {
      throw new Error(`starter layer ${i} has unknown kind ${String(kind)}`);
    }
    return {
      name,
      kind,
      suggestedContent:
        typeof suggestedContent === "string" ? suggestedContent : null,
    };
  });
  return { artboardPreset, palette: palette as string[], starterLayers };
}

type Mode = "themed" | "plan";

type Phase =
  | { kind: "idle" }
  | { kind: "asking" }
  | { kind: "preview"; plan: BriefPlan }
  | { kind: "applying" }
  | { kind: "error"; message: string };

export function BriefModal({
  open,
  onClose,
  llmReady = false,
  onApplied,
}: BriefModalProps): JSX.Element | null {
  const [brief, setBrief] = useState("");
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });
  const [presets, setPresets] = useState<readonly ArtboardPreset[]>([]);
  const [mode, setMode] = useState<Mode>("themed");

  // Themed-generator controls.
  const [format, setFormat] = useState<ThemedDesignFormat>("deck");
  const [themeId, setThemeId] = useState<ThemeId>("midnight");
  const [onePagerSize, setOnePagerSize] = useState<ThemedOnePagerSize>("a4");
  const [sectionCount, setSectionCount] = useState<number | null>(null);
  const [useLlm, setUseLlm] = useState(false);

  // Fetch the real preset list whenever the modal is opened so the
  // SYSTEM_PROMPT enumerates names that the Rust bridge will actually
  // accept. The previous hardcoded list drifted from
  // `standard_presets()` and broke the end-to-end flow.
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    void window.kcreate.artboard
      .presets()
      .then((list) => {
        if (!cancelled) setPresets(list);
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          const message = err instanceof Error ? err.message : String(err);
          setPhase({ kind: "error", message: `preset fetch failed: ${message}` });
        }
      });
    return () => {
      cancelled = true;
    };
  }, [open]);

  const presetKeys = useMemo(
    () => new Set(presets.map((p) => normalizePresetName(p.name))),
    [presets],
  );

  const switchMode = useCallback((next: Mode) => {
    setMode(next);
    setPhase({ kind: "idle" });
  }, []);

  // Deck and one-pager offer different section-count ranges, so a
  // value picked for one format can be out of range for the other
  // (the bridge clamps it, but the <select> would then show a stale,
  // unmatched value). Reset to "Auto" on a format switch to keep the
  // control's displayed value always valid.
  const switchFormat = useCallback((next: ThemedDesignFormat) => {
    setFormat(next);
    setSectionCount(null);
  }, []);

  // Shared: guarantee an open workspace before any bridge mutation.
  // The Rust apply paths mutate the currently mounted workspace; when
  // the modal is opened from `HomePage` no project is open yet so we
  // materialise a fresh scratch one (same convention as the "Create
  // new" tile). When opened from inside the editor a workspace already
  // exists and we compose onto it instead of replacing it.
  const ensureProject = useCallback(async () => {
    const current = await window.kcreate.document.getProjectInfo();
    if (current === null) {
      await openScratchProject();
    }
  }, []);

  const generateThemed = useCallback(async () => {
    if (brief.trim().length === 0) return;
    setPhase({ kind: "applying" });
    try {
      await ensureProject();
      const options: ThemedDesignOptions = {
        format,
        themeId,
        useLlm: useLlm && llmReady,
      };
      if (format === "onePager") {
        options.onePagerSize = onePagerSize;
      }
      if (sectionCount !== null) {
        options.sectionCount = sectionCount;
      }
      const result = await window.kcreate.phase10.aiGenerateThemedDesign(
        brief,
        options,
      );
      // Reset before notifying: the parent typically navigates and
      // unmounts this modal, so post-`onApplied` state writes would be
      // pointless. Resetting first means a re-mount sees a clean state.
      setBrief("");
      setPhase({ kind: "idle" });
      onApplied(result);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setPhase({ kind: "error", message });
    }
  }, [
    brief,
    ensureProject,
    format,
    themeId,
    onePagerSize,
    sectionCount,
    useLlm,
    llmReady,
    onApplied,
  ]);

  const submitBrief = useCallback(async () => {
    if (presets.length === 0) {
      setPhase({
        kind: "error",
        message: "artboard presets not loaded yet — try again in a moment",
      });
      return;
    }
    setPhase({ kind: "asking" });
    try {
      const messages: LlmMessage[] = [
        { role: "system", content: buildSystemPrompt(presets) },
        { role: "user", content: brief },
      ];
      const reply = await window.kcreate.llm.chat(messages, 1024, 0.2);
      const plan = parseBriefPlan(reply.content, presetKeys);
      setPhase({ kind: "preview", plan });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setPhase({ kind: "error", message });
    }
  }, [brief, presets, presetKeys]);

  const applyPlan = useCallback(
    async (plan: BriefPlan) => {
      setPhase({ kind: "applying" });
      try {
        await ensureProject();
        const result = await window.kcreate.phase9.briefToProject(plan);
        setBrief("");
        setPhase({ kind: "idle" });
        onApplied(result);
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        setPhase({ kind: "error", message });
      }
    },
    [ensureProject, onApplied],
  );

  if (!open) return null;

  const busy = phase.kind === "asking" || phase.kind === "applying";

  return (
    <div style={overlayStyle} role="dialog" aria-modal="true">
      <div style={dialogStyle} data-testid="kcreate-brief-modal">
        <header style={headerStyle}>
          <h2 style={titleStyle}>Start from a brief</h2>
          <button
            type="button"
            onClick={onClose}
            style={iconButtonStyle}
            aria-label="Close"
            data-testid="kcreate-brief-close"
          >
            ×
          </button>
        </header>

        <div style={modeRowStyle} role="tablist" aria-label="Generation mode">
          <button
            type="button"
            role="tab"
            aria-selected={mode === "themed"}
            onClick={() => switchMode("themed")}
            disabled={busy}
            style={mode === "themed" ? modeTabActiveStyle : modeTabStyle}
            data-testid="kcreate-brief-mode-themed"
          >
            Themed design
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={mode === "plan"}
            onClick={() => switchMode("plan")}
            disabled={busy || !llmReady}
            title={
              llmReady
                ? undefined
                : "Start the local LLM in Model Manager to enable this mode"
            }
            style={mode === "plan" ? modeTabActiveStyle : modeTabStyle}
            data-testid="kcreate-brief-mode-plan"
          >
            Single artboard
          </button>
        </div>

        <p style={helpTextStyle}>
          {mode === "themed" ? (
            <>
              Describe your topic and pick a format + theme. KCreate lays
              out a complete, themed multi-page design you can refine.
              Works offline; turn on <strong>Expand with AI</strong> to let
              the local model flesh out the content.
            </>
          ) : (
            <>
              Describe the design you want. The local model will propose an
              artboard size, a palette, and a few starter layers. Nothing
              is created until you click <strong>Apply</strong>.
            </>
          )}
        </p>

        <textarea
          value={brief}
          onChange={(e) => setBrief(e.target.value)}
          placeholder={
            mode === "themed"
              ? "e.g. Pitch deck for an indie coffee roaster opening its first café."
              : "e.g. A friendly poster for a coffee-shop grand opening on Saturday."
          }
          rows={4}
          style={textareaStyle}
          disabled={busy}
          data-testid="kcreate-brief-textarea"
        />

        {mode === "themed" && (
          <ThemedControls
            format={format}
            onFormat={switchFormat}
            themeId={themeId}
            onTheme={setThemeId}
            onePagerSize={onePagerSize}
            onOnePagerSize={setOnePagerSize}
            sectionCount={sectionCount}
            onSectionCount={setSectionCount}
            useLlm={useLlm}
            onUseLlm={setUseLlm}
            llmReady={llmReady}
            disabled={busy}
          />
        )}

        {phase.kind === "error" && (
          <p role="alert" style={errorStyle} data-testid="kcreate-brief-error">
            {phase.message}
          </p>
        )}
        {mode === "plan" && phase.kind === "preview" && (
          <BriefPreview plan={phase.plan} />
        )}

        <footer style={footerStyle}>
          {mode === "themed" ? (
            <>
              <button
                type="button"
                onClick={onClose}
                style={secondaryButtonStyle}
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => void generateThemed()}
                disabled={brief.trim().length === 0 || busy}
                style={primaryButtonStyle}
                data-testid="kcreate-themed-generate"
              >
                {phase.kind === "applying" ? "Generating…" : "Generate"}
              </button>
            </>
          ) : phase.kind === "preview" ? (
            <>
              <button
                type="button"
                onClick={() => setPhase({ kind: "idle" })}
                style={secondaryButtonStyle}
              >
                Discard
              </button>
              <button
                type="button"
                onClick={() => void applyPlan(phase.plan)}
                style={primaryButtonStyle}
                data-testid="kcreate-brief-apply"
              >
                Apply
              </button>
            </>
          ) : (
            <>
              <button type="button" onClick={onClose} style={secondaryButtonStyle}>
                Cancel
              </button>
              <button
                type="button"
                onClick={() => void submitBrief()}
                disabled={brief.trim().length === 0 || busy}
                style={primaryButtonStyle}
                data-testid="kcreate-brief-submit"
              >
                {phase.kind === "asking"
                  ? "Thinking…"
                  : phase.kind === "applying"
                    ? "Creating project…"
                    : "Generate plan"}
              </button>
            </>
          )}
        </footer>
      </div>
    </div>
  );
}

function ThemedControls({
  format,
  onFormat,
  themeId,
  onTheme,
  onePagerSize,
  onOnePagerSize,
  sectionCount,
  onSectionCount,
  useLlm,
  onUseLlm,
  llmReady,
  disabled,
}: {
  format: ThemedDesignFormat;
  onFormat: (f: ThemedDesignFormat) => void;
  themeId: ThemeId;
  onTheme: (t: ThemeId) => void;
  onePagerSize: ThemedOnePagerSize;
  onOnePagerSize: (s: ThemedOnePagerSize) => void;
  sectionCount: number | null;
  onSectionCount: (n: number | null) => void;
  useLlm: boolean;
  onUseLlm: (v: boolean) => void;
  llmReady: boolean;
  disabled: boolean;
}): JSX.Element {
  return (
    <div style={controlsStyle} data-testid="kcreate-themed-controls">
      <div style={controlRowStyle}>
        <span style={controlLabelStyle}>Format</span>
        <div style={segmentStyle}>
          <button
            type="button"
            onClick={() => onFormat("deck")}
            disabled={disabled}
            aria-pressed={format === "deck"}
            style={format === "deck" ? segmentButtonActiveStyle : segmentButtonStyle}
            data-testid="kcreate-themed-format-deck"
          >
            Deck
          </button>
          <button
            type="button"
            onClick={() => onFormat("onePager")}
            disabled={disabled}
            aria-pressed={format === "onePager"}
            style={
              format === "onePager" ? segmentButtonActiveStyle : segmentButtonStyle
            }
            data-testid="kcreate-themed-format-onepager"
          >
            One-pager
          </button>
        </div>
      </div>

      <div style={controlRowStyle}>
        <span style={controlLabelStyle}>Theme</span>
        <div style={themeRowStyle}>
          {THEME_OPTIONS.map((t) => (
            <button
              key={t.id}
              type="button"
              onClick={() => onTheme(t.id)}
              disabled={disabled}
              aria-pressed={themeId === t.id}
              title={t.label}
              style={{
                ...themeChipStyle,
                outline:
                  themeId === t.id ? `2px solid ${colors.accent}` : "none",
                outlineOffset: 2,
              }}
              data-testid={`kcreate-themed-theme-${t.id}`}
            >
              <span style={themeChipSwatchesStyle}>
                {t.swatches.map((c, i) => (
                  <span
                    key={`${t.id}-${i}`}
                    style={{ ...themeChipSwatchStyle, background: c }}
                  />
                ))}
              </span>
              <span style={themeChipLabelStyle}>{t.label}</span>
            </button>
          ))}
        </div>
      </div>

      <div style={controlRowInlineStyle}>
        {format === "onePager" && (
          <label style={inlineFieldStyle}>
            <span style={controlLabelStyle}>Page size</span>
            <select
              value={onePagerSize}
              onChange={(e) =>
                onOnePagerSize(e.target.value as ThemedOnePagerSize)
              }
              disabled={disabled}
              style={selectStyle}
              data-testid="kcreate-themed-size"
            >
              <option value="a4">A4</option>
              <option value="letter">Letter</option>
              <option value="square">Square</option>
            </select>
          </label>
        )}
        <label style={inlineFieldStyle}>
          <span style={controlLabelStyle}>
            {format === "deck" ? "Slides" : "Sections"}
          </span>
          <select
            value={sectionCount === null ? "auto" : String(sectionCount)}
            onChange={(e) =>
              onSectionCount(
                e.target.value === "auto" ? null : Number(e.target.value),
              )
            }
            disabled={disabled}
            style={selectStyle}
            data-testid="kcreate-themed-sections"
          >
            <option value="auto">Auto</option>
            {(format === "deck"
              ? [4, 5, 6, 7, 8, 9, 10]
              : [3, 4, 5, 6]
            ).map((n) => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
          </select>
        </label>
      </div>

      <label
        style={{
          ...checkboxRowStyle,
          opacity: llmReady ? 1 : 0.55,
        }}
        title={
          llmReady
            ? "Use the local model to expand the brief into richer content"
            : "Start the local LLM in Model Manager to enable AI enrichment"
        }
      >
        <input
          type="checkbox"
          checked={useLlm && llmReady}
          onChange={(e) => onUseLlm(e.target.checked)}
          disabled={disabled || !llmReady}
          data-testid="kcreate-themed-usellm"
        />
        <span>Expand with AI {llmReady ? "" : "(model not loaded)"}</span>
      </label>
    </div>
  );
}

function BriefPreview({ plan }: { plan: BriefPlan }): JSX.Element {
  return (
    <div style={previewStyle} data-testid="kcreate-brief-preview">
      <div style={previewRowStyle}>
        <span style={previewLabelStyle}>Artboard</span>
        <span>{plan.artboardPreset}</span>
      </div>
      <div style={previewRowStyle}>
        <span style={previewLabelStyle}>Palette</span>
        <div style={swatchRowStyle}>
          {plan.palette.map((c, i) => (
            // Key includes the index so duplicate colors in an LLM-
            // generated palette (e.g. two `#1F2937`s) don't collide and
            // trigger React's "duplicate key" warning + render skips.
            <span
              key={`${i}-${c}`}
              title={c}
              style={{ ...swatchStyle, background: c }}
            />
          ))}
        </div>
      </div>
      <div style={previewRowStyle}>
        <span style={previewLabelStyle}>Starter layers</span>
        <ul style={layerListStyle}>
          {plan.starterLayers.map((l, i) => (
            <li key={i} style={layerItemStyle}>
              <strong>{l.name}</strong>
              <span style={layerKindStyle}> ({l.kind})</span>
              {l.suggestedContent !== null && (
                <span style={layerContentStyle}> — {l.suggestedContent}</span>
              )}
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}

const overlayStyle: React.CSSProperties = {
  position: "fixed",
  inset: 0,
  background: "rgba(0,0,0,0.45)",
  zIndex: 200,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
};

const dialogStyle: React.CSSProperties = {
  width: 540,
  maxWidth: "90vw",
  maxHeight: "85vh",
  overflowY: "auto",
  background: colors.bg,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
  padding: spacing.lg,
  fontFamily: font.family,
  color: colors.text,
  display: "flex",
  flexDirection: "column",
  gap: spacing.md,
};

const headerStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
};

const titleStyle: React.CSSProperties = { margin: 0, fontSize: 18, fontWeight: 600 };

const iconButtonStyle: React.CSSProperties = {
  background: "transparent",
  border: "none",
  fontSize: 24,
  color: colors.textMuted,
  cursor: "pointer",
  padding: 0,
  lineHeight: 1,
};

const modeRowStyle: React.CSSProperties = {
  display: "flex",
  gap: spacing.xs,
  background: colors.bgSoft,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  padding: 3,
};

const modeTabStyle: React.CSSProperties = {
  flex: 1,
  background: "transparent",
  color: colors.textMuted,
  border: "none",
  borderRadius: radius.sm,
  padding: "7px 10px",
  fontSize: 13,
  fontWeight: 600,
  cursor: "pointer",
};

const modeTabActiveStyle: React.CSSProperties = {
  ...modeTabStyle,
  background: colors.bg,
  color: colors.text,
  boxShadow: "0 1px 2px rgba(0,0,0,0.18)",
};

const helpTextStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 13,
  color: colors.textMuted,
};

const textareaStyle: React.CSSProperties = {
  fontFamily: font.family,
  fontSize: 13,
  padding: spacing.sm,
  background: colors.bgSoft,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  color: colors.text,
  resize: "vertical",
};

const controlsStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.md,
  padding: spacing.md,
  background: colors.bgSoft,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
};

const controlRowStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.xs,
};

const controlRowInlineStyle: React.CSSProperties = {
  display: "flex",
  gap: spacing.md,
  flexWrap: "wrap",
};

const inlineFieldStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.xs,
};

const controlLabelStyle: React.CSSProperties = {
  fontSize: 11,
  textTransform: "uppercase",
  letterSpacing: 0.5,
  color: colors.textMuted,
};

const segmentStyle: React.CSSProperties = {
  display: "flex",
  gap: spacing.xs,
};

const segmentButtonStyle: React.CSSProperties = {
  flex: 1,
  background: colors.bg,
  color: colors.text,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  padding: "7px 12px",
  fontSize: 13,
  cursor: "pointer",
};

const segmentButtonActiveStyle: React.CSSProperties = {
  ...segmentButtonStyle,
  background: colors.accent,
  color: "white",
  border: `1px solid ${colors.accent}`,
  fontWeight: 600,
};

const themeRowStyle: React.CSSProperties = {
  display: "flex",
  gap: spacing.sm,
  flexWrap: "wrap",
};

const themeChipStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  alignItems: "center",
  gap: 4,
  background: colors.bg,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  padding: 6,
  cursor: "pointer",
};

const themeChipSwatchesStyle: React.CSSProperties = {
  display: "flex",
  borderRadius: 4,
  overflow: "hidden",
  border: `1px solid ${colors.border}`,
};

const themeChipSwatchStyle: React.CSSProperties = {
  width: 18,
  height: 24,
};

const themeChipLabelStyle: React.CSSProperties = {
  fontSize: 11,
  color: colors.text,
};

const selectStyle: React.CSSProperties = {
  fontFamily: font.family,
  fontSize: 13,
  padding: "6px 8px",
  background: colors.bg,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  color: colors.text,
};

const checkboxRowStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: spacing.xs,
  fontSize: 13,
  color: colors.text,
  cursor: "pointer",
};

const errorStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 13,
  color: "#B91C1C",
};

const previewStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.sm,
  padding: spacing.md,
  background: colors.bgSoft,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
};

const previewRowStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.xs,
};

const previewLabelStyle: React.CSSProperties = {
  fontSize: 11,
  textTransform: "uppercase",
  letterSpacing: 0.5,
  color: colors.textMuted,
};

const swatchRowStyle: React.CSSProperties = {
  display: "flex",
  gap: 6,
};

const swatchStyle: React.CSSProperties = {
  width: 24,
  height: 24,
  borderRadius: 4,
  border: `1px solid ${colors.border}`,
};

const layerListStyle: React.CSSProperties = {
  margin: 0,
  paddingLeft: spacing.md,
  fontSize: 13,
  display: "flex",
  flexDirection: "column",
  gap: 2,
};

const layerItemStyle: React.CSSProperties = { color: colors.text };
const layerKindStyle: React.CSSProperties = { color: colors.textMuted };
const layerContentStyle: React.CSSProperties = { color: colors.textMuted };

const footerStyle: React.CSSProperties = {
  display: "flex",
  justifyContent: "flex-end",
  gap: spacing.sm,
};

const primaryButtonStyle: React.CSSProperties = {
  background: colors.accent,
  color: "white",
  border: "none",
  borderRadius: radius.sm,
  padding: "8px 14px",
  fontWeight: 600,
  cursor: "pointer",
};

const secondaryButtonStyle: React.CSSProperties = {
  background: "transparent",
  color: colors.text,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  padding: "8px 14px",
  cursor: "pointer",
};

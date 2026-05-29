// Phase 9 Block B Task 7 — "Start from a brief" modal.
//
// PROPOSAL.md §4.1: "A 'Start from a brief' tile that opens a local
// LLM prompt; the model fills out an artboard preset, palette, and
// starter layers."
//
// Flow:
//   1. User opens the modal from the Home page and types a brief
//      (e.g. "I need a poster for a coffee-shop grand opening").
//   2. We hit the local LLM sidecar via `window.kcreate.llm.chat`
//      with a prompt that asks for STRICT JSON matching
//      `BriefPlan` (see shared/scene.ts).
//   3. The reply is parsed + validated; on success we hand the
//      plan to `window.kcreate.phase9.briefToProject` which
//      atomically creates the project, applies the brand kit,
//      and lays down the starter layers. The bridge returns the
//      new project id + artboard id which the parent uses to
//      navigate into the editor.
//
// The user can always cancel; nothing is persisted until the
// bridge call completes successfully.

import { useCallback, useState } from "react";
import type {
  BriefApplyResult,
  BriefPlan,
  LlmMessage,
} from "../../../shared/scene";
import { openScratchProject } from "../lib/scratchProject";
import { colors, font, radius, spacing } from "../styles/tokens";

interface BriefModalProps {
  open: boolean;
  onClose: () => void;
  /** Fired with the bridge result so the shell can navigate. */
  onApplied: (result: BriefApplyResult) => void;
}

const SYSTEM_PROMPT = `You are an expert graphic-design planner. The
user will describe a design they want and you will reply with STRICT
JSON in this shape — no Markdown, no commentary, no extra fields:

{
  "artboardPreset": string,      // one of: "instagramPost",
                                 // "instagramStory", "twitterPost",
                                 // "facebookPost", "youtubeThumbnail",
                                 // "a4", "letter", "businessCard",
                                 // "desktop1440", "mobile375",
                                 // "logo1024"
  "palette": string[],           // 3-6 hex colors like "#1F2937"
  "starterLayers": [
    { "name": string, "kind": "text"|"shape"|"image"|"group",
      "suggestedContent": string|null }
  ]
}

Pick the artboardPreset that best matches the user's brief. Pick a
palette that fits the brand mood. Suggest 3-8 starter layers (title,
subtitle, hero shape, etc.). Reply ONLY with the JSON object.`;

const KNOWN_PRESETS = new Set([
  "instagramPost",
  "instagramStory",
  "twitterPost",
  "facebookPost",
  "youtubeThumbnail",
  "a4",
  "letter",
  "businessCard",
  "desktop1440",
  "mobile375",
  "logo1024",
]);

function parseBriefPlan(raw: string): BriefPlan {
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
  if (typeof artboardPreset !== "string" || !KNOWN_PRESETS.has(artboardPreset)) {
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

type Phase =
  | { kind: "idle" }
  | { kind: "asking" }
  | { kind: "preview"; plan: BriefPlan }
  | { kind: "applying" }
  | { kind: "error"; message: string };

export function BriefModal({
  open,
  onClose,
  onApplied,
}: BriefModalProps): JSX.Element | null {
  const [brief, setBrief] = useState("");
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });

  const submitBrief = useCallback(async () => {
    setPhase({ kind: "asking" });
    try {
      const messages: LlmMessage[] = [
        { role: "system", content: SYSTEM_PROMPT },
        { role: "user", content: brief },
      ];
      const reply = await window.kcreate.llm.chat(messages, 1024, 0.2);
      const plan = parseBriefPlan(reply.content);
      setPhase({ kind: "preview", plan });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setPhase({ kind: "error", message });
    }
  }, [brief]);

  const applyPlan = useCallback(async (plan: BriefPlan) => {
    setPhase({ kind: "applying" });
    try {
      // The Rust `brief_to_project` bridge mutates the currently
      // mounted workspace. When the modal is opened from
      // `HomePage` no project is yet open, so we materialise a
      // fresh scratch project before applying the plan — same
      // scratch convention used by the "Create new" tile in
      // `App.handleOpenEditor`. When the modal is opened from
      // inside the editor a workspace is already mounted and we
      // skip the scratch step so the brief composes onto the
      // existing project instead of replacing it.
      const current = await window.kcreate.document.getProjectInfo();
      if (current === null) {
        await openScratchProject();
      }
      const result = await window.kcreate.phase9.briefToProject(plan);
      // Reset local state *before* notifying the parent. The parent
      // typically navigates to the editor in response to
      // `onApplied`, which unmounts this modal — calling
      // `setBrief` / `setPhase` afterwards is a no-op on the
      // unmounted component (silent in React 18, but pointless).
      // Resetting first means a subsequent re-mount sees a clean
      // initial state without depending on React 18 semantics.
      setBrief("");
      setPhase({ kind: "idle" });
      onApplied(result);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setPhase({ kind: "error", message });
    }
  }, [onApplied]);

  if (!open) return null;

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
        <p style={helpTextStyle}>
          Describe the design you want. The local model will propose
          an artboard size, a palette, and a few starter layers. Nothing
          is created until you click <strong>Apply</strong>.
        </p>
        <textarea
          value={brief}
          onChange={(e) => setBrief(e.target.value)}
          placeholder="e.g. A friendly poster for a coffee-shop grand opening on Saturday."
          rows={5}
          style={textareaStyle}
          disabled={phase.kind === "asking" || phase.kind === "applying"}
          data-testid="kcreate-brief-textarea"
        />
        {phase.kind === "error" && (
          <p role="alert" style={errorStyle} data-testid="kcreate-brief-error">
            {phase.message}
          </p>
        )}
        {phase.kind === "preview" && (
          <BriefPreview plan={phase.plan} />
        )}
        <footer style={footerStyle}>
          {phase.kind === "preview" ? (
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
              <button
                type="button"
                onClick={onClose}
                style={secondaryButtonStyle}
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => void submitBrief()}
                disabled={
                  brief.trim().length === 0 ||
                  phase.kind === "asking" ||
                  phase.kind === "applying"
                }
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

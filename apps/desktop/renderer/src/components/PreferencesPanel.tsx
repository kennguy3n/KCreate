// PreferencesPanel — Phase 10 Block D Task 23.
//
// Workspace-wide preferences UI. Loads `~/.kcreate/preferences.json`
// through `window.kcreate.phase10.preferencesLoad`, edits in place,
// and writes back via `preferencesSave`. Sections mirror the Rust
// `Preferences` struct in `kcreate_bridge::phase10` and the wire
// mirror in `apps/desktop/shared/scene.ts` — adding a field here
// without updating both sides will fail typecheck.

import { useCallback, useEffect, useRef, useState } from "react";

import type { Preferences } from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface PreferencesPanelProps {
  onClose: () => void;
  onStatus?: (msg: string | null) => void;
}

type Section = "general" | "canvas" | "ai" | "performance" | "privacy";

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

export function PreferencesPanel({
  onClose,
  onStatus,
}: PreferencesPanelProps): JSX.Element {
  const [prefs, setPrefs] = useState<Preferences | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [section, setSection] = useState<Section>("general");
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState(false);

  // Stable ref so requestClose always sees the latest `dirty` value
  // without re-creating the callback (PanelShell memoizes on it).
  const dirtyRef = useRef(dirty);
  useEffect(() => {
    dirtyRef.current = dirty;
  }, [dirty]);

  // Guard close paths: if there are unsaved edits, ask the user
  // before discarding. Wired into the footer Close button, the
  // PanelShell header × button, and the overlay click. UX finding
  // from Devin Review (ANALYSIS_…_0007).
  const requestClose = useCallback(() => {
    if (
      dirtyRef.current &&
      !window.confirm(
        "You have unsaved preference changes. Discard them and close?",
      )
    ) {
      return;
    }
    onClose();
  }, [onClose]);

  const load = useCallback(async () => {
    try {
      const p = await window.kcreate.phase10.preferencesLoad();
      setPrefs(p);
      setLoadError(null);
      setDirty(false);
    } catch (e) {
      setLoadError(errMsg(e));
      onStatus?.(`preferences load failed: ${errMsg(e)}`);
    }
  }, [onStatus]);

  useEffect(() => {
    void load();
  }, [load]);

  const save = useCallback(async () => {
    if (!prefs) return;
    setBusy(true);
    try {
      await window.kcreate.phase10.preferencesSave(prefs);
      setDirty(false);
      onStatus?.("preferences saved");
    } catch (e) {
      onStatus?.(`preferences save failed: ${errMsg(e)}`);
    } finally {
      setBusy(false);
    }
  }, [prefs, onStatus]);

  const update = useCallback((next: Preferences) => {
    setPrefs(next);
    setDirty(true);
  }, []);

  const resetCorrupted = useCallback(async () => {
    setBusy(true);
    try {
      // Re-loading after the user OKs the prompt would just hit the
      // same parse error; instead push an empty defaults payload
      // through save to overwrite the corrupted file in place.
      const defaultPrefs: Preferences = {
        general: {
          theme: "system",
          language: "en-US",
          autosaveIntervalSec: 60,
          scratchProjectCleanupDays: 30,
        },
        canvas: {
          defaultGridSpacing: 16,
          defaultGridSubdivisions: 4,
          snapThresholdPx: 6,
          rulerUnits: "px",
        },
        ai: {
          defaultLlmModel: "",
          autoStartSidecar: false,
          gbnfGrammarDebugging: false,
        },
        performance: {
          rasterCacheBudgetMb: 512,
          undoDepthOverride: null,
          lowResourceMode: false,
        },
        privacy: {
          telemetryOptIn: false,
          auditLogRetentionDays: 90,
        },
      };
      await window.kcreate.phase10.preferencesSave(defaultPrefs);
      setPrefs(defaultPrefs);
      setLoadError(null);
      setDirty(false);
      onStatus?.("preferences reset to defaults");
    } catch (e) {
      onStatus?.(`reset failed: ${errMsg(e)}`);
    } finally {
      setBusy(false);
    }
  }, [onStatus]);

  if (loadError) {
    return (
      <PanelShell onClose={requestClose} title="Preferences">
        <div
          style={{
            padding: spacing.lg,
            color: colors.danger,
            background: colors.bgSoft,
            borderRadius: radius.md,
            border: `1px solid ${colors.border}`,
          }}
        >
          <div style={{ fontWeight: 600, marginBottom: spacing.sm }}>
            Preferences file is corrupted
          </div>
          <div
            style={{
              fontSize: 12,
              fontFamily: "monospace",
              marginBottom: spacing.md,
              wordBreak: "break-word",
            }}
          >
            {loadError}
          </div>
          <p style={{ fontSize: 13, color: colors.textMuted }}>
            Your preferences file at <code>~/.kcreate/preferences.json</code>{" "}
            could not be parsed. You can either fix it manually or reset to
            defaults.
          </p>
          <button
            type="button"
            onClick={() => void resetCorrupted()}
            disabled={busy}
            style={{
              marginTop: spacing.md,
              padding: `${spacing.sm}px ${spacing.md}px`,
              background: colors.accent,
              color: colors.textInverse,
              border: "none",
              borderRadius: radius.sm,
              cursor: busy ? "not-allowed" : "pointer",
            }}
          >
            Reset to defaults
          </button>
        </div>
      </PanelShell>
    );
  }

  if (!prefs) {
    return (
      <PanelShell onClose={requestClose} title="Preferences">
        <div style={{ padding: spacing.lg, color: colors.textMuted }}>
          Loading…
        </div>
      </PanelShell>
    );
  }

  return (
    <PanelShell onClose={requestClose} title="Preferences">
      <div style={{ display: "flex", height: "100%" }}>
        <nav
          style={{
            width: 140,
            borderRight: `1px solid ${colors.border}`,
            padding: spacing.sm,
            display: "flex",
            flexDirection: "column",
            gap: 2,
          }}
        >
          {(
            ["general", "canvas", "ai", "performance", "privacy"] as Section[]
          ).map((s) => (
            <button
              key={s}
              type="button"
              onClick={() => setSection(s)}
              style={{
                padding: `${spacing.sm}px ${spacing.md}px`,
                background:
                  section === s ? colors.accentBgSoft : "transparent",
                color: section === s ? colors.accent : colors.text,
                border: "none",
                borderRadius: radius.sm,
                cursor: "pointer",
                textAlign: "left",
                textTransform: "capitalize",
                fontWeight: section === s ? 600 : 400,
              }}
            >
              {s}
            </button>
          ))}
        </nav>
        <div style={{ flex: 1, padding: spacing.lg, overflow: "auto" }}>
          {section === "general" && (
            <GeneralSection prefs={prefs} onChange={update} />
          )}
          {section === "canvas" && (
            <CanvasSection prefs={prefs} onChange={update} />
          )}
          {section === "ai" && <AiSection prefs={prefs} onChange={update} />}
          {section === "performance" && (
            <PerformanceSection prefs={prefs} onChange={update} />
          )}
          {section === "privacy" && (
            <PrivacySection prefs={prefs} onChange={update} />
          )}
        </div>
      </div>
      <footer
        style={{
          padding: spacing.md,
          borderTop: `1px solid ${colors.border}`,
          display: "flex",
          justifyContent: "flex-end",
          gap: spacing.sm,
        }}
      >
        <button
          type="button"
          onClick={requestClose}
          style={{
            padding: `${spacing.sm}px ${spacing.md}px`,
            background: "transparent",
            color: colors.text,
            border: `1px solid ${colors.border}`,
            borderRadius: radius.sm,
            cursor: "pointer",
          }}
        >
          Close
        </button>
        <button
          type="button"
          onClick={() => void save()}
          disabled={!dirty || busy}
          style={{
            padding: `${spacing.sm}px ${spacing.md}px`,
            background: dirty ? colors.accent : colors.bgSoft,
            color: dirty ? colors.textInverse : colors.textMuted,
            border: "none",
            borderRadius: radius.sm,
            cursor: dirty && !busy ? "pointer" : "not-allowed",
          }}
        >
          {busy ? "Saving…" : dirty ? "Save changes" : "Saved"}
        </button>
      </footer>
    </PanelShell>
  );
}

interface SectionProps {
  prefs: Preferences;
  onChange: (next: Preferences) => void;
}

function GeneralSection({ prefs, onChange }: SectionProps): JSX.Element {
  return (
    <Group label="General">
      <Field label="Theme">
        <select
          value={prefs.general.theme}
          onChange={(e) =>
            onChange({
              ...prefs,
              general: {
                ...prefs.general,
                theme: e.target.value as Preferences["general"]["theme"],
              },
            })
          }
        >
          <option value="system">System</option>
          <option value="light">Light</option>
          <option value="dark">Dark</option>
        </select>
      </Field>
      <Field label="Language">
        <input
          type="text"
          value={prefs.general.language}
          onChange={(e) =>
            onChange({
              ...prefs,
              general: { ...prefs.general, language: e.target.value },
            })
          }
        />
      </Field>
      <Field label="Autosave interval (sec)">
        <NumberInput
          value={prefs.general.autosaveIntervalSec}
          min={5}
          max={3600}
          onChange={(v) =>
            onChange({
              ...prefs,
              general: { ...prefs.general, autosaveIntervalSec: v },
            })
          }
        />
      </Field>
      <Field
        label="Scratch project retention (days)"
        hint="0 disables the autosaver's scratch-project sweep."
      >
        <NumberInput
          value={prefs.general.scratchProjectCleanupDays}
          min={0}
          max={365}
          onChange={(v) =>
            onChange({
              ...prefs,
              general: { ...prefs.general, scratchProjectCleanupDays: v },
            })
          }
        />
      </Field>
    </Group>
  );
}

function CanvasSection({ prefs, onChange }: SectionProps): JSX.Element {
  return (
    <Group label="Canvas">
      <Field label="Default grid spacing (px)">
        <NumberInput
          value={prefs.canvas.defaultGridSpacing}
          min={1}
          max={512}
          step={1}
          onChange={(v) =>
            onChange({
              ...prefs,
              canvas: { ...prefs.canvas, defaultGridSpacing: v },
            })
          }
        />
      </Field>
      <Field label="Default grid subdivisions">
        <NumberInput
          value={prefs.canvas.defaultGridSubdivisions}
          min={1}
          max={32}
          onChange={(v) =>
            onChange({
              ...prefs,
              canvas: { ...prefs.canvas, defaultGridSubdivisions: v },
            })
          }
        />
      </Field>
      <Field label="Snap threshold (px)">
        <NumberInput
          value={prefs.canvas.snapThresholdPx}
          min={0}
          max={64}
          onChange={(v) =>
            onChange({
              ...prefs,
              canvas: { ...prefs.canvas, snapThresholdPx: v },
            })
          }
        />
      </Field>
      <Field label="Ruler units">
        <select
          value={prefs.canvas.rulerUnits}
          onChange={(e) =>
            onChange({
              ...prefs,
              canvas: {
                ...prefs.canvas,
                rulerUnits: e.target
                  .value as Preferences["canvas"]["rulerUnits"],
              },
            })
          }
        >
          <option value="px">px</option>
          <option value="mm">mm</option>
          <option value="in">in</option>
          <option value="pt">pt</option>
        </select>
      </Field>
    </Group>
  );
}

function AiSection({ prefs, onChange }: SectionProps): JSX.Element {
  return (
    <Group label="AI">
      <Field label="Default LLM model">
        <input
          type="text"
          value={prefs.ai.defaultLlmModel}
          onChange={(e) =>
            onChange({
              ...prefs,
              ai: { ...prefs.ai, defaultLlmModel: e.target.value },
            })
          }
          placeholder="e.g. qwen2.5-coder-7b.Q4_K_M.gguf"
        />
      </Field>
      <Toggle
        label="Auto-start LLM sidecar on app launch"
        value={prefs.ai.autoStartSidecar}
        onChange={(v) =>
          onChange({
            ...prefs,
            ai: { ...prefs.ai, autoStartSidecar: v },
          })
        }
      />
      <Toggle
        label="GBNF grammar debugging (logs every grammar miss)"
        value={prefs.ai.gbnfGrammarDebugging}
        onChange={(v) =>
          onChange({
            ...prefs,
            ai: { ...prefs.ai, gbnfGrammarDebugging: v },
          })
        }
      />
    </Group>
  );
}

function PerformanceSection({ prefs, onChange }: SectionProps): JSX.Element {
  return (
    <Group label="Performance">
      <Field label="Raster cache budget (MB)">
        <NumberInput
          value={prefs.performance.rasterCacheBudgetMb}
          min={64}
          max={8192}
          step={64}
          onChange={(v) =>
            onChange({
              ...prefs,
              performance: {
                ...prefs.performance,
                rasterCacheBudgetMb: v,
              },
            })
          }
        />
      </Field>
      <Field
        label="Undo depth override"
        hint="Leave blank to use the per-project default."
      >
        <NumberInput
          value={prefs.performance.undoDepthOverride ?? 0}
          min={0}
          max={1000}
          onChange={(v) =>
            onChange({
              ...prefs,
              performance: {
                ...prefs.performance,
                undoDepthOverride: v === 0 ? null : v,
              },
            })
          }
        />
      </Field>
      <Toggle
        label="Low-resource mode (smaller caches, deferred init)"
        value={prefs.performance.lowResourceMode}
        onChange={(v) =>
          onChange({
            ...prefs,
            performance: { ...prefs.performance, lowResourceMode: v },
          })
        }
      />
    </Group>
  );
}

function PrivacySection({ prefs, onChange }: SectionProps): JSX.Element {
  return (
    <Group label="Privacy">
      <Toggle
        label="Telemetry opt-in (always off by default)"
        value={prefs.privacy.telemetryOptIn}
        onChange={(v) =>
          onChange({
            ...prefs,
            privacy: { ...prefs.privacy, telemetryOptIn: v },
          })
        }
      />
      <Field label="Audit-log retention (days)">
        <NumberInput
          value={prefs.privacy.auditLogRetentionDays}
          min={0}
          max={3650}
          onChange={(v) =>
            onChange({
              ...prefs,
              privacy: {
                ...prefs.privacy,
                auditLogRetentionDays: v,
              },
            })
          }
        />
      </Field>
    </Group>
  );
}

function Group({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}): JSX.Element {
  return (
    <section style={{ marginBottom: spacing.lg }}>
      <h3
        style={{
          margin: 0,
          marginBottom: spacing.md,
          fontSize: 13,
          textTransform: "uppercase",
          letterSpacing: 1.2,
          color: colors.textMuted,
        }}
      >
        {label}
      </h3>
      <div style={{ display: "flex", flexDirection: "column", gap: spacing.md }}>
        {children}
      </div>
    </section>
  );
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}): JSX.Element {
  return (
    <label
      style={{
        display: "flex",
        flexDirection: "column",
        gap: 4,
        fontSize: 13,
      }}
    >
      <span style={{ fontWeight: 500 }}>{label}</span>
      {children}
      {hint ? (
        <span style={{ fontSize: 11, color: colors.textMuted }}>{hint}</span>
      ) : null}
    </label>
  );
}

function NumberInput({
  value,
  min,
  max,
  step = 1,
  onChange,
}: {
  value: number;
  min?: number;
  max?: number;
  step?: number;
  onChange: (v: number) => void;
}): JSX.Element {
  return (
    <input
      type="number"
      value={value}
      min={min}
      max={max}
      step={step}
      onChange={(e) => {
        const n = Number.parseFloat(e.target.value);
        if (Number.isFinite(n)) onChange(n);
      }}
      style={{
        padding: spacing.sm,
        borderRadius: radius.sm,
        border: `1px solid ${colors.border}`,
        background: colors.bg,
        color: colors.text,
      }}
    />
  );
}

function Toggle({
  label,
  value,
  onChange,
}: {
  label: string;
  value: boolean;
  onChange: (v: boolean) => void;
}): JSX.Element {
  return (
    <label
      style={{
        display: "flex",
        alignItems: "center",
        gap: spacing.sm,
        fontSize: 13,
        cursor: "pointer",
      }}
    >
      <input
        type="checkbox"
        checked={value}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span>{label}</span>
    </label>
  );
}

function PanelShell({
  onClose,
  title,
  children,
}: {
  onClose: () => void;
  title: string;
  children: React.ReactNode;
}): JSX.Element {
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={title}
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(0,0,0,0.55)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        zIndex: 1000,
      }}
      onClick={onClose}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          width: "min(720px, 95vw)",
          height: "min(560px, 90vh)",
          background: colors.bg,
          border: `1px solid ${colors.border}`,
          borderRadius: radius.card,
          boxShadow: "0 24px 48px rgba(0,0,0,0.35)",
          overflow: "hidden",
          display: "flex",
          flexDirection: "column",
        }}
      >
        <header
          style={{
            padding: spacing.md,
            borderBottom: `1px solid ${colors.border}`,
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
          }}
        >
          <h2 style={{ margin: 0, fontSize: 16 }}>{title}</h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close preferences"
            style={{
              background: "transparent",
              color: colors.text,
              border: "none",
              fontSize: 18,
              cursor: "pointer",
              padding: 4,
            }}
          >
            ×
          </button>
        </header>
        <div style={{ flex: 1, overflow: "hidden" }}>{children}</div>
      </div>
    </div>
  );
}

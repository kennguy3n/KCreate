// ColorSettingsPanel — surface the document-level Phase 2 color
// management settings (working RGB space, optional CMYK profile,
// rendering intent, soft-proof, gamut warning) and let the user
// commit changes through `window.kcreate.color`.
//
// Activating a CMYK working space here is what flips the PDF
// exporter into `DeviceCMYK` operator mode (see the
// `color_to_printpdf` mapper in `kcreate_export::pdf`). Toggling
// soft-proof or gamut warning here is what enables the
// `SoftProofOverlay` over the canvas.

import { useCallback, useEffect, useState } from "react";

import type {
  ColorSettings,
  IccProfile,
  RenderingIntent,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

const RGB_PROFILES: ReadonlyArray<{ value: IccProfile; label: string }> = [
  { value: "SrgbIec61966", label: "sRGB (IEC 61966-2-1)" },
  { value: "AdobeRgb1998", label: "Adobe RGB (1998)" },
  { value: "DisplayP3", label: "Display P3" },
];

// `null` is rendered as the explicit "no CMYK output" option so
// users have to opt in before any CMYK conversion runs.
const CMYK_PROFILES: ReadonlyArray<{
  value: IccProfile | null;
  label: string;
}> = [
  { value: null, label: "None (RGB output only)" },
  { value: "FogRa39", label: "FOGRA39 (Europe coated)" },
  { value: "Swop2006", label: "SWOP 2006 (US web coated)" },
];

const INTENTS: ReadonlyArray<{ value: RenderingIntent; label: string }> = [
  { value: "Perceptual", label: "Perceptual" },
  { value: "RelativeColorimetric", label: "Relative colorimetric" },
  { value: "Saturation", label: "Saturation" },
  { value: "AbsoluteColorimetric", label: "Absolute colorimetric" },
];

const SOFT_PROOF_PROFILES: ReadonlyArray<{
  value: IccProfile | null;
  label: string;
}> = [
  { value: null, label: "Off" },
  { value: "FogRa39", label: "FOGRA39 (Europe coated)" },
  { value: "Swop2006", label: "SWOP 2006 (US web coated)" },
  { value: "AdobeRgb1998", label: "Adobe RGB" },
  { value: "DisplayP3", label: "Display P3" },
];

const APPLY_TO_NEW_KEY = "kcreate.color.applyToNewDocuments";

export interface ColorSettingsPanelProps {
  /** Bubble status text up to the editor's global status strip. */
  onStatus?: (msg: string | null) => void;
}

export function ColorSettingsPanel({
  onStatus,
}: ColorSettingsPanelProps): JSX.Element {
  const [settings, setSettings] = useState<ColorSettings | null>(null);
  const [busy, setBusy] = useState(false);
  const [applyToNew, setApplyToNew] = useState<boolean>(() => {
    try {
      return window.localStorage.getItem(APPLY_TO_NEW_KEY) === "1";
    } catch {
      return false;
    }
  });
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setError(null);
    try {
      const next = await window.kcreate.color.getSettings();
      setSettings(next);
    } catch (e) {
      setError(errMsg(e));
    }
  }, []);

  // Subscribe to push-broadcast settings changes so the panel stays in
  // sync with the document state when another path mutates the
  // settings — most importantly an undo / redo of a
  // `color_settings_update` op (see
  // `apps/desktop/main/src/main.ts::broadcastForCommand`). Without this
  // subscription the panel would keep displaying the optimistic
  // `setSettings(next)` value from `commit` even after the workspace
  // reverted, matching the stale-dropdown bug Devin Review flagged on
  // commit 7b207ed.
  useEffect(() => {
    void load();
    const unsubscribe = window.kcreate.color.onSettingsChanged(() => {
      void load();
    });
    return () => {
      unsubscribe();
    };
  }, [load]);

  const commit = useCallback(
    async (next: ColorSettings) => {
      setBusy(true);
      onStatus?.("Color: updating settings…");
      try {
        await window.kcreate.color.updateSettings(next);
        setSettings(next);
        onStatus?.("Color: settings updated.");
        setError(null);
      } catch (e) {
        const msg = errMsg(e);
        setError(msg);
        onStatus?.(`Color: update failed — ${msg}`);
      } finally {
        setBusy(false);
      }
    },
    [onStatus],
  );

  const toggleApplyToNew = useCallback((v: boolean) => {
    setApplyToNew(v);
    try {
      window.localStorage.setItem(APPLY_TO_NEW_KEY, v ? "1" : "0");
    } catch {
      // Storage unavailable (private mode, quota exceeded): the
      // toggle still updates the in-memory state for the session.
    }
  }, []);

  if (!settings) {
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
        <h3 style={{ margin: 0, fontSize: 13, fontWeight: 600 }}>
          Color management
        </h3>
        {error ? (
          <p style={{ color: severityColor("error"), fontSize: 12, margin: 0 }}>
            {error}
          </p>
        ) : (
          <p style={{ color: colors.textMuted, fontSize: 12, margin: 0 }}>
            Loading color settings…
          </p>
        )}
      </div>
    );
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: spacing.md }}>
      <h3 style={{ margin: 0, fontSize: 13, fontWeight: 600 }}>
        Color management
      </h3>
      <SelectField
        label="Working RGB space"
        value={profileKey(settings.working_space_rgb)}
        options={RGB_PROFILES.map((p) => ({
          value: profileKey(p.value),
          label: p.label,
        }))}
        onChange={(key) => {
          const profile = profileByKey(RGB_PROFILES, key);
          if (profile) {
            void commit({ ...settings, working_space_rgb: profile });
          }
        }}
        disabled={busy}
      />
      <SelectField
        label="Working CMYK profile"
        value={profileKey(settings.working_space_cmyk)}
        options={CMYK_PROFILES.map((p) => ({
          value: profileKey(p.value),
          label: p.label,
        }))}
        onChange={(key) => {
          const profile = profileByKey(CMYK_PROFILES, key);
          // `profileByKey` returns `null` for the "None" entry and
          // `undefined` for an unknown key. Only the explicit `null`
          // means "clear the CMYK profile".
          if (profile === undefined) return;
          void commit({ ...settings, working_space_cmyk: profile });
        }}
        disabled={busy}
      />
      <SelectField
        label="Rendering intent"
        value={settings.rendering_intent}
        options={INTENTS.map((i) => ({ value: i.value, label: i.label }))}
        onChange={(value) => {
          void commit({
            ...settings,
            rendering_intent: value as RenderingIntent,
          });
        }}
        disabled={busy}
      />
      <SelectField
        label="Soft-proof profile"
        value={profileKey(settings.soft_proof_profile)}
        options={SOFT_PROOF_PROFILES.map((p) => ({
          value: profileKey(p.value),
          label: p.label,
        }))}
        onChange={(key) => {
          const profile = profileByKey(SOFT_PROOF_PROFILES, key);
          if (profile === undefined) return;
          void commit({ ...settings, soft_proof_profile: profile });
        }}
        disabled={busy}
      />
      <CheckboxField
        label="Gamut warning (highlight out-of-gamut pixels)"
        checked={settings.gamut_warning}
        onChange={(v) => {
          void commit({ ...settings, gamut_warning: v });
        }}
        disabled={busy}
      />
      <CheckboxField
        label="Apply to new documents"
        checked={applyToNew}
        onChange={toggleApplyToNew}
        disabled={busy}
      />
      <p style={{ color: colors.textMuted, fontSize: 11, margin: 0 }}>
        CMYK output is OFF until a working CMYK profile is selected.
        Authored CMYK fills round-trip through PDF export without
        K-channel loss.
      </p>
      {error ? (
        <p style={{ color: severityColor("error"), fontSize: 12, margin: 0 }}>
          {error}
        </p>
      ) : null}
    </div>
  );
}

// Both well-known and custom ICC profile variants are serialized as
// either a bare string (`"SrgbIec61966"`) or a tagged object
// (`{ "Custom": ... }`). The dropdown uses the bare string as a
// stable key; custom profiles are not yet wired in Phase 2 but we
// keep the type handling correct so adding them later is a UI-only
// change.
function profileKey(p: IccProfile | null): string {
  if (p === null) return "__none__";
  if (typeof p === "string") return p;
  return `Custom:${p.Custom.blob_hash}`;
}

function profileByKey<T extends { value: IccProfile | null; label: string }>(
  options: ReadonlyArray<T>,
  key: string,
): IccProfile | null | undefined {
  for (const opt of options) {
    if (profileKey(opt.value) === key) return opt.value;
  }
  return undefined;
}

function SelectField({
  label,
  value,
  options,
  onChange,
  disabled,
}: {
  label: string;
  value: string;
  options: ReadonlyArray<{ value: string; label: string }>;
  onChange: (v: string) => void;
  disabled?: boolean;
}): JSX.Element {
  return (
    <label style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      <span style={{ fontSize: 11, color: colors.textMuted }}>{label}</span>
      <select
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
        style={{
          padding: "4px 6px",
          fontSize: 12,
          border: `1px solid ${colors.border}`,
          borderRadius: radius.card,
          background: colors.bg,
          color: colors.text,
        }}
      >
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
    </label>
  );
}

function CheckboxField({
  label,
  checked,
  onChange,
  disabled,
}: {
  label: string;
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
}): JSX.Element {
  return (
    <label
      style={{
        display: "flex",
        alignItems: "center",
        gap: spacing.xs,
      }}
    >
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span style={{ fontSize: 11, color: colors.textMuted }}>{label}</span>
    </label>
  );
}

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  return JSON.stringify(e);
}

function severityColor(s: "error"): string {
  if (s === "error") return "#DC2626";
  return colors.textMuted;
}

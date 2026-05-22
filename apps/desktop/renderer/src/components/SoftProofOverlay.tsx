// SoftProofOverlay — when soft-proof or gamut-warning mode is on,
// drop a CSS overlay over the canvas to simulate the target output
// profile and (optionally) highlight out-of-gamut pixels.
//
// Phase 2 uses a simplified CSS-filter-based simulation:
//   - CMYK proof profiles desaturate + slightly warm the canvas so
//     designers can see the perceptual loss before exporting.
//   - Wide-gamut RGB proof profiles (Display P3, Adobe RGB) apply a
//     gentle saturation boost so designers can preview the extra
//     range without firing the actual GPU ICC transform.
// The full ICC transform (lcms2 or a pure-Rust equivalent) ships in
// Phase 3 — see PROGRESS.md.
//
// The gamut-warning overlay paints a faint bright-green wash on top
// of the canvas as a placeholder until the renderer can mark
// out-of-gamut pixels per-fragment. The wash uses `mix-blend-mode`
// so it only shows up where the canvas already has saturated color.

import { useEffect, useState } from "react";

import type { ColorSettings, IccProfile } from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface SoftProofOverlayProps {
  /**
   * Initial settings (optional). The overlay refetches on mount to
   * stay in sync with `ColorSettingsPanel` edits coming through
   * `window.kcreate.color`.
   */
  initial?: ColorSettings;
  /**
   * Polling interval in ms. The Phase 2 bridge does not push
   * settings changes, so the overlay polls on a slow cadence to
   * pick up edits from the settings panel. Set to `0` to disable.
   */
  pollMs?: number;
}

export function SoftProofOverlay({
  initial,
  pollMs = 2000,
}: SoftProofOverlayProps): JSX.Element | null {
  const [settings, setSettings] = useState<ColorSettings | null>(
    initial ?? null,
  );

  useEffect(() => {
    let alive = true;
    let timer: ReturnType<typeof setInterval> | null = null;

    const tick = async () => {
      try {
        const next = await window.kcreate.color.getSettings();
        if (alive) setSettings(next);
      } catch {
        // Bridge may not be ready before the first render; the next
        // tick retries.
      }
    };

    void tick();
    if (pollMs > 0) {
      timer = setInterval(() => {
        void tick();
      }, pollMs);
    }
    return () => {
      alive = false;
      if (timer !== null) clearInterval(timer);
    };
  }, [pollMs]);

  if (!settings) return null;
  const softProof = settings.soft_proof_profile;
  const gamutWarn = settings.gamut_warning;
  if (!softProof && !gamutWarn) return null;

  const filter = softProof ? cssFilterForProfile(softProof) : "none";
  return (
    <>
      {softProof ? (
        <div
          aria-hidden
          style={{
            position: "absolute",
            inset: 0,
            pointerEvents: "none",
            // Backdrop-filter applies the simulation to whatever the
            // canvas painted underneath, so we don't interfere with
            // the actual scene buffer that exports use.
            backdropFilter: filter,
            WebkitBackdropFilter: filter,
          }}
        />
      ) : null}
      {gamutWarn ? (
        <div
          aria-hidden
          style={{
            position: "absolute",
            inset: 0,
            pointerEvents: "none",
            // Photoshop-style bright-green wash on top of saturated
            // colors. `screen` only lightens — neutrals stay neutral
            // so a clean grayscale layout shows no warning.
            background:
              "linear-gradient(rgba(0, 255, 0, 0.18), rgba(0, 255, 0, 0.18))",
            mixBlendMode: "screen",
          }}
        />
      ) : null}
      <Badge profile={softProof} gamutWarn={gamutWarn} />
    </>
  );
}

/// Map an ICC profile to a CSS filter chain that approximates the
/// perceptual loss of converting into that profile. Real ICC
/// transforms ship in Phase 3.
function cssFilterForProfile(profile: IccProfile): string {
  // Custom profiles fall back to a mild desaturation since we don't
  // know the gamut without parsing the embedded blob.
  if (typeof profile !== "string") {
    return "saturate(0.85)";
  }
  switch (profile) {
    case "FogRa39":
    case "Swop2006":
      // CMYK presses can't hit saturated sRGB blues / greens; clip
      // them by desaturating and pulling slightly warm.
      return "saturate(0.7) sepia(0.08)";
    case "AdobeRgb1998":
      // Wide-gamut RGB: gently boost saturation so designers see
      // the extra range without firing the full ICC transform.
      return "saturate(1.1)";
    case "DisplayP3":
      return "saturate(1.15)";
    case "SrgbIec61966":
      // Soft-proofing into the working space is a no-op.
      return "none";
    default:
      return "saturate(0.85)";
  }
}

function Badge({
  profile,
  gamutWarn,
}: {
  profile: IccProfile | null;
  gamutWarn: boolean;
}): JSX.Element {
  return (
    <div
      style={{
        position: "absolute",
        top: spacing.sm,
        left: spacing.sm,
        background: "rgba(17, 24, 39, 0.85)",
        color: colors.textInverse,
        fontSize: 11,
        padding: "2px 8px",
        borderRadius: radius.pill,
        pointerEvents: "none",
        display: "flex",
        gap: 6,
      }}
    >
      {profile ? <span>Soft Proof: {labelForProfile(profile)}</span> : null}
      {gamutWarn ? (
        <span style={{ color: "#86EFAC", fontWeight: 600 }}>
          Gamut Warning
        </span>
      ) : null}
    </div>
  );
}

function labelForProfile(p: IccProfile): string {
  if (typeof p !== "string") return p.Custom.name;
  switch (p) {
    case "SrgbIec61966":
      return "sRGB";
    case "AdobeRgb1998":
      return "Adobe RGB";
    case "DisplayP3":
      return "Display P3";
    case "FogRa39":
      return "FOGRA39";
    case "Swop2006":
      return "SWOP 2006";
    default:
      return p;
  }
}

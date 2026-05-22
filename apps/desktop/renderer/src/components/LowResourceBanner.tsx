import { useState } from "react";

import type { ResourceLimits } from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface LowResourceBannerProps {
  limits: ResourceLimits;
  /**
   * Called when the user flips the low-resource flag. Tier 0 hosts
   * will ignore `false` (the Rust side pins the flag); the parent
   * should re-fetch limits after the call to display the actual
   * resolved state.
   */
  onToggle: (enabled: boolean) => Promise<void> | void;
}

/**
 * Subtle banner shown at the bottom of the editor whenever
 * low-resource mode is active. Surfaces effective limits and offers
 * an inline expandable Settings drawer so the user can flip the
 * mode off (where the tier permits) and see exactly which budgets
 * shrank.
 */
export function LowResourceBanner({
  limits,
  onToggle,
}: LowResourceBannerProps): JSX.Element | null {
  const [expanded, setExpanded] = useState(false);

  if (!limits.lowResourceMode) {
    return null;
  }

  return (
    <div
      style={{
        borderTop: `1px solid ${colors.border}`,
        background: colors.bgSoft,
        color: colors.text,
        fontSize: 11,
      }}
      role="status"
      aria-live="polite"
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: spacing.md,
          padding: `${spacing.xs}px ${spacing.md}px`,
        }}
      >
        <span style={{ fontWeight: 600 }}>Low-resource mode</span>
        <span style={{ color: colors.textMuted }}>
          Tier <code>{limits.deviceTier}</code> — undo depth{" "}
          <b>{limits.effectiveUndoDepth}</b>, raster cache{" "}
          <b>{limits.effectiveRasterCacheMb} MB</b>, max model{" "}
          <b>{limits.effectiveMaxModelMb} MB</b>,{" "}
          {limits.gpuRenderingAllowed ? "GPU rendering on" : "GPU rendering off"}
        </span>
        <button
          type="button"
          onClick={() => setExpanded((s) => !s)}
          style={inlineLinkStyle}
        >
          {expanded ? "Hide settings" : "Settings"}
        </button>
      </div>
      {expanded ? (
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            gap: spacing.xs,
            padding: `${spacing.sm}px ${spacing.md}px ${spacing.md}px`,
            borderTop: `1px dashed ${colors.border}`,
          }}
        >
          <p style={{ margin: 0, color: colors.textMuted, lineHeight: 1.5 }}>
            KCreate trims undo depth, raster caching, AI model size and GPU
            rendering on smaller machines so the editor stays responsive.
            Tier 0 devices keep low-resource mode on at all times.
          </p>
          <button
            type="button"
            onClick={() => {
              void onToggle(false);
            }}
            style={primaryButtonStyle}
          >
            Try disabling low-resource mode
          </button>
        </div>
      ) : null}
    </div>
  );
}

const inlineLinkStyle: React.CSSProperties = {
  marginLeft: "auto",
  padding: "2px 8px",
  border: "none",
  background: "transparent",
  color: colors.accent,
  fontSize: 11,
  fontWeight: 600,
  borderRadius: radius.pill,
  cursor: "pointer",
};

const primaryButtonStyle: React.CSSProperties = {
  alignSelf: "flex-start",
  padding: "4px 10px",
  fontSize: 11,
  fontWeight: 600,
  background: "transparent",
  color: colors.accent,
  border: `1px solid ${colors.accent}`,
  borderRadius: radius.pill,
  cursor: "pointer",
};

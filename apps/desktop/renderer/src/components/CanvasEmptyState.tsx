// Empty-canvas call-to-action (workstream H1).
//
// Shown centred over the canvas when the project has no artboards
// yet. Instead of a blank void, it routes the user straight into the
// real creation flows (template, AI generate, browse elements) and
// reminds them of the command palette. Purely presentational: the
// parent supplies the same `DiscoveryAction` handlers the welcome
// overlay and command palette use.

import { colors, radius, spacing } from "../styles/tokens";
import type { DiscoveryAction } from "./DiscoveryWelcome";
import { Icon } from "./Icon";
import { useI18n } from "../i18n";

export interface CanvasEmptyStateProps {
  /** Pre-formatted command-palette hint (e.g. `"Ctrl K"`). */
  paletteHint: string;
  /** Opens the command palette. */
  onOpenPalette: () => void;
  /** Discovery flows surfaced as buttons, in priority order. */
  actions: ReadonlyArray<DiscoveryAction>;
}

/**
 * Centred empty-state panel for a project with no artboards. The
 * overlay root is `pointer-events: none` so it never blocks canvas
 * gestures around the panel; the panel itself opts back in so its
 * buttons remain clickable.
 */
export function CanvasEmptyState({
  paletteHint,
  onOpenPalette,
  actions,
}: CanvasEmptyStateProps): JSX.Element {
  const { t } = useI18n();
  // The lead carries a `{hint}` marker where the palette shortcut
  // should appear as a styled <kbd>; split on it so the keystroke
  // stays a real element while the surrounding copy is fully
  // translatable and the marker can sit anywhere a translator needs
  // it (including in RTL locales).
  const [leadHead, leadTail = ""] = t("canvasEmpty.lead").split("{hint}");
  return (
    <div style={overlayStyle} data-testid="kcreate-canvas-empty-state">
      <div style={panelStyle}>
        <span aria-hidden="true" style={badgeStyle}>
          <Icon name="sparkles" size={22} />
        </span>
        <h2 style={titleStyle}>{t("canvasEmpty.title")}</h2>
        <p style={leadStyle}>
          {leadHead}
          <kbd style={kbdStyle}>{paletteHint}</kbd>
          {leadTail}
        </p>
        <div style={actionsRowStyle}>
          {actions.map((action, index) => (
            <button
              key={action.id}
              type="button"
              onClick={action.run}
              style={index === 0 ? primaryButtonStyle : secondaryButtonStyle}
              data-testid={`kcreate-canvas-empty-action-${action.id}`}
            >
              <Icon name={action.icon} size={16} />
              {action.label}
            </button>
          ))}
        </div>
        <button
          type="button"
          onClick={onOpenPalette}
          style={paletteLinkStyle}
          data-testid="kcreate-canvas-empty-palette"
        >
          <Icon name="command" size={14} />
          {t("canvasEmpty.openPalette")}
        </button>
      </div>
    </div>
  );
}

// -----------------------------------------------------------------------------
// Styles
// -----------------------------------------------------------------------------

const overlayStyle: React.CSSProperties = {
  position: "absolute",
  inset: 0,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  pointerEvents: "none",
  padding: spacing.lg,
};

const panelStyle: React.CSSProperties = {
  pointerEvents: "auto",
  maxWidth: 460,
  width: "100%",
  background: colors.bg,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
  padding: spacing.lg,
  display: "flex",
  flexDirection: "column",
  alignItems: "center",
  textAlign: "center",
  gap: spacing.sm,
  boxShadow: "0 12px 32px rgba(0, 0, 0, 0.18)",
};

const badgeStyle: React.CSSProperties = {
  width: 48,
  height: 48,
  borderRadius: 14,
  background: colors.accent,
  color: "white",
  display: "inline-flex",
  alignItems: "center",
  justifyContent: "center",
  marginBottom: spacing.xs,
};

const titleStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 18,
  fontWeight: 600,
  color: colors.text,
};

const leadStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 13,
  color: colors.textMuted,
  lineHeight: 1.5,
};

const actionsRowStyle: React.CSSProperties = {
  display: "flex",
  flexWrap: "wrap",
  justifyContent: "center",
  gap: spacing.sm,
  marginTop: spacing.sm,
};

const buttonBase: React.CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: spacing.xs,
  borderRadius: radius.sm,
  padding: "8px 14px",
  fontSize: 13,
  fontWeight: 600,
  cursor: "pointer",
};

const primaryButtonStyle: React.CSSProperties = {
  ...buttonBase,
  background: colors.accent,
  color: "white",
  border: "none",
};

const secondaryButtonStyle: React.CSSProperties = {
  ...buttonBase,
  background: "transparent",
  color: colors.text,
  border: `1px solid ${colors.border}`,
};

const paletteLinkStyle: React.CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: spacing.xs,
  background: "transparent",
  border: "none",
  color: colors.textMuted,
  fontSize: 12,
  cursor: "pointer",
  marginTop: spacing.xs,
};

const kbdStyle: React.CSSProperties = {
  fontFamily: "inherit",
  fontSize: 11,
  fontWeight: 600,
  color: colors.textMuted,
  background: colors.bgSoft,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  padding: "1px 6px",
};

// First-run discovery welcome (workstream H1).
//
// A lightweight, localStorage-gated overlay shown the first time a
// user lands in the editor. Its whole job is discoverability: point
// the newcomer at the command palette (the one keystroke that reaches
// everything) and the three headline G-wave flows — start from a
// template, generate with AI, and browse the elements library.
//
// This is deliberately presentational: the parent owns the `open`
// gate (see `lib/discoveryWelcome.ts`) and supplies the real handlers
// via `actions` so each card routes into the SAME flow the toolbar /
// command palette already drives — there are no bespoke menu items
// here. It is separate from the bridge-backed `WelcomeModal` (the
// AI-model installer on the HomePage), which solves a different
// problem.

import { useEffect } from "react";

import { colors, radius, spacing } from "../styles/tokens";
import { Icon, type IconName } from "./Icon";

/**
 * One discovery flow surfaced as a card in the welcome overlay and the
 * empty-canvas state. `run` is the real handler (the parent wires it
 * to `openTemplates` / `openAiGenerate` / `openElements` etc.) so
 * there is exactly one implementation of each action.
 */
export interface DiscoveryAction {
  id: string;
  label: string;
  description: string;
  icon: IconName;
  run: () => void;
}

export interface DiscoveryWelcomeProps {
  /** Controlled visibility — the parent owns the first-run gate. */
  open: boolean;
  /**
   * Pre-formatted command-palette shortcut hint (e.g. `"Ctrl K"`),
   * derived from the live binding so it tracks user rebinds.
   */
  paletteHint: string;
  /** Opens the command palette — the headline call to action. */
  onOpenPalette: () => void;
  /** Discovery flows surfaced as cards, in priority order. */
  actions: ReadonlyArray<DiscoveryAction>;
  /**
   * Fired on every close path (action picked, palette opened, skip,
   * Esc, backdrop click). The parent persists the "seen" marker and
   * flips `open` to false.
   */
  onDismiss: () => void;
}

/**
 * The first-run discovery overlay. Renders nothing when `open` is
 * false. Any action (including opening the palette) also dismisses,
 * so the welcome never lingers on top of the flow the user just
 * chose.
 */
export function DiscoveryWelcome({
  open,
  paletteHint,
  onOpenPalette,
  actions,
  onDismiss,
}: DiscoveryWelcomeProps): JSX.Element | null {
  // Esc dismisses, matching every other overlay in the editor. Bound
  // only while open so we don't hold a listener for a hidden modal.
  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        event.preventDefault();
        onDismiss();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
    };
  }, [open, onDismiss]);

  if (!open) return null;

  const runAction = (action: DiscoveryAction): void => {
    // Dismiss first so the "seen" marker is written before the flow
    // opens its own overlay (e.g. the template picker) on top.
    onDismiss();
    action.run();
  };

  const openPalette = (): void => {
    onDismiss();
    onOpenPalette();
  };

  return (
    <div
      style={overlayStyle}
      role="dialog"
      aria-modal="true"
      aria-labelledby="kcreate-discovery-title"
      data-testid="kcreate-discovery-welcome"
      onClick={(event) => {
        if (event.target === event.currentTarget) onDismiss();
      }}
    >
      <div style={dialogStyle}>
        <header style={headerStyle}>
          <div>
            <h2 id="kcreate-discovery-title" style={titleStyle}>
              Welcome to KCreate
            </h2>
            <p style={leadStyle}>
              Everything is one keystroke away. Press the command
              palette to jump to any tool, panel, or flow.
            </p>
          </div>
          <button
            type="button"
            onClick={onDismiss}
            style={iconButtonStyle}
            aria-label="Dismiss welcome"
            data-testid="kcreate-discovery-close"
          >
            ×
          </button>
        </header>

        <button
          type="button"
          onClick={openPalette}
          style={paletteButtonStyle}
          data-testid="kcreate-discovery-palette"
        >
          <span style={paletteButtonLabelStyle}>
            <Icon name="command" size={16} />
            Open the command palette
          </span>
          <kbd style={kbdStyle}>{paletteHint}</kbd>
        </button>

        <div style={cardGridStyle}>
          {actions.map((action) => (
            <button
              key={action.id}
              type="button"
              onClick={() => runAction(action)}
              style={cardStyle}
              data-testid={`kcreate-discovery-action-${action.id}`}
            >
              <span aria-hidden="true" style={cardIconStyle}>
                <Icon name={action.icon} size={18} />
              </span>
              <span style={cardLabelStyle}>{action.label}</span>
              <span style={cardDescStyle}>{action.description}</span>
            </button>
          ))}
        </div>

        <footer style={footerStyle}>
          <button
            type="button"
            onClick={onDismiss}
            style={secondaryButtonStyle}
            data-testid="kcreate-discovery-skip"
          >
            Maybe later
          </button>
        </footer>
      </div>
    </div>
  );
}

// -----------------------------------------------------------------------------
// Styles
// -----------------------------------------------------------------------------

const overlayStyle: React.CSSProperties = {
  position: "fixed",
  inset: 0,
  background: "rgba(0,0,0,0.45)",
  zIndex: 240,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
};

const dialogStyle: React.CSSProperties = {
  width: 560,
  maxWidth: "92vw",
  maxHeight: "85vh",
  overflowY: "auto",
  background: colors.bg,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
  padding: spacing.lg,
  color: colors.text,
  display: "flex",
  flexDirection: "column",
  gap: spacing.md,
};

const headerStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "flex-start",
  justifyContent: "space-between",
  gap: spacing.md,
};

const titleStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 18,
  fontWeight: 600,
};

const leadStyle: React.CSSProperties = {
  margin: `${spacing.xs}px 0 0`,
  fontSize: 13,
  color: colors.textMuted,
  lineHeight: 1.45,
};

const iconButtonStyle: React.CSSProperties = {
  background: "transparent",
  border: "none",
  fontSize: 24,
  color: colors.textMuted,
  cursor: "pointer",
  padding: 0,
  lineHeight: 1,
};

const paletteButtonStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  gap: spacing.sm,
  background: colors.accentBgSoft,
  border: `1px solid ${colors.accentRing}`,
  borderRadius: radius.md,
  padding: `${spacing.sm}px ${spacing.md}px`,
  cursor: "pointer",
  color: colors.text,
  fontSize: 14,
  fontWeight: 600,
};

const paletteButtonLabelStyle: React.CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: spacing.sm,
};

const kbdStyle: React.CSSProperties = {
  fontFamily: "inherit",
  fontSize: 12,
  fontWeight: 600,
  color: colors.textMuted,
  background: colors.bg,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  padding: "2px 8px",
};

const cardGridStyle: React.CSSProperties = {
  display: "grid",
  gridTemplateColumns: "repeat(auto-fit, minmax(150px, 1fr))",
  gap: spacing.sm,
};

const cardStyle: React.CSSProperties = {
  textAlign: "left",
  display: "flex",
  flexDirection: "column",
  gap: spacing.xs,
  background: colors.bgSoft,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
  padding: spacing.md,
  cursor: "pointer",
  color: colors.text,
};

const cardIconStyle: React.CSSProperties = {
  width: 36,
  height: 36,
  borderRadius: 10,
  background: colors.accent,
  color: "white",
  display: "inline-flex",
  alignItems: "center",
  justifyContent: "center",
  marginBottom: spacing.xs,
};

const cardLabelStyle: React.CSSProperties = {
  fontSize: 14,
  fontWeight: 600,
  color: colors.text,
};

const cardDescStyle: React.CSSProperties = {
  fontSize: 12,
  color: colors.textMuted,
  lineHeight: 1.4,
};

const footerStyle: React.CSSProperties = {
  display: "flex",
  justifyContent: "flex-end",
  gap: spacing.sm,
};

const secondaryButtonStyle: React.CSSProperties = {
  background: "transparent",
  color: colors.text,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  padding: "8px 14px",
  cursor: "pointer",
};

import { colors, font, radius, spacing } from "../styles/tokens";

export type EditorMode =
  | "design"
  | "vector"
  | "image"
  | "layout"
  | "prototype"
  | "inspect"
  | "export";

export const EDITOR_MODES: ReadonlyArray<{
  mode: EditorMode;
  label: string;
}> = [
  { mode: "design", label: "Design" },
  { mode: "vector", label: "Vector" },
  { mode: "image", label: "Image" },
  { mode: "layout", label: "Layout" },
  { mode: "prototype", label: "Prototype" },
  { mode: "inspect", label: "Inspect" },
  { mode: "export", label: "Export" },
];

export interface TopBarProps {
  projectName: string;
  mode: EditorMode;
  onModeChange: (mode: EditorMode) => void;
  canUndo: boolean;
  canRedo: boolean;
  onUndo: () => void;
  onRedo: () => void;
  onExport: () => void;
  onBackHome: () => void;
}

export function TopBar(props: TopBarProps): JSX.Element {
  const {
    projectName,
    mode,
    onModeChange,
    canUndo,
    canRedo,
    onUndo,
    onRedo,
    onExport,
    onBackHome,
  } = props;
  return (
    <header
      style={{
        display: "flex",
        alignItems: "center",
        gap: spacing.md,
        padding: `${spacing.sm}px ${spacing.md}px`,
        background: colors.bg,
        borderBottom: `1px solid ${colors.border}`,
        fontFamily: font.family,
        color: colors.text,
        fontSize: 13,
      }}
    >
      <button
        type="button"
        onClick={onBackHome}
        style={pillButton(false)}
        aria-label="Back to home"
      >
        ← Home
      </button>
      <span style={{ fontWeight: 600 }}>{projectName}</span>
      <nav
        style={{
          display: "flex",
          gap: spacing.xs,
          marginLeft: spacing.md,
        }}
        aria-label="Editor mode"
      >
        {EDITOR_MODES.map(({ mode: m, label }) => (
          <button
            key={m}
            type="button"
            onClick={() => onModeChange(m)}
            style={modeTab(m === mode)}
            aria-pressed={m === mode}
          >
            {label}
          </button>
        ))}
      </nav>
      <div style={{ flex: 1 }} />
      <button
        type="button"
        onClick={onUndo}
        disabled={!canUndo}
        style={pillButton(false, !canUndo)}
      >
        Undo
      </button>
      <button
        type="button"
        onClick={onRedo}
        disabled={!canRedo}
        style={pillButton(false, !canRedo)}
      >
        Redo
      </button>
      <button type="button" onClick={onExport} style={pillButton(true)}>
        Export
      </button>
    </header>
  );
}

function pillButton(primary: boolean, disabled = false): React.CSSProperties {
  return {
    border: `1px solid ${primary ? colors.accent : colors.border}`,
    background: primary ? colors.accent : colors.bg,
    color: primary ? colors.textInverse : colors.text,
    borderRadius: radius.pill,
    padding: "6px 14px",
    fontSize: 12,
    fontWeight: 500,
    cursor: disabled ? "not-allowed" : "pointer",
    opacity: disabled ? 0.5 : 1,
    transition: "background 120ms ease",
  };
}

function modeTab(active: boolean): React.CSSProperties {
  return {
    border: "1px solid transparent",
    background: active ? colors.bgSoft : "transparent",
    color: active ? colors.accent : colors.textMuted,
    borderRadius: radius.pill,
    padding: "4px 12px",
    fontSize: 12,
    fontWeight: active ? 600 : 500,
  };
}

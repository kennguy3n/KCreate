import { colors, font, radius, spacing } from "../styles/tokens";
import type { ToolId } from "../pages/EditorPage";

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

/// The tool palette each mode exposes. The mode switcher uses this
/// to decide which tool to default to when the user switches modes,
/// and the TopBar uses it to enable/disable tool buttons.
const TOOLS_BY_MODE: Record<EditorMode, ReadonlyArray<ToolId>> = {
  design: ["select", "rect", "ellipse", "line", "text"],
  vector: ["select", "rect", "ellipse", "line"],
  image: ["select"],
  layout: ["select", "rect", "text"],
  prototype: ["select"],
  inspect: ["select"],
  export: ["select"],
};

export function toolsForMode(mode: EditorMode): ReadonlyArray<ToolId> {
  return TOOLS_BY_MODE[mode];
}

/// Which right-panel face to show by default when this mode becomes
/// active. The Inspect mode reuses the properties panel for now —
/// Phase 1 will add a dedicated inspect face.
export type RightPanelFocus = "properties" | "ai" | "export" | "inspect";

const PANEL_FOR_MODE: Record<EditorMode, RightPanelFocus> = {
  design: "properties",
  vector: "properties",
  image: "ai",
  layout: "properties",
  prototype: "properties",
  inspect: "inspect",
  export: "export",
};

export function defaultPanelForMode(mode: EditorMode): RightPanelFocus {
  return PANEL_FOR_MODE[mode];
}

const TOOL_LABELS: Record<ToolId, { label: string; key: string }> = {
  select: { label: "Select", key: "V" },
  rect: { label: "Rect", key: "R" },
  ellipse: { label: "Ellipse", key: "E" },
  line: { label: "Line", key: "L" },
  text: { label: "Text", key: "T" },
};

export interface TopBarProps {
  projectName: string;
  mode: EditorMode;
  onModeChange: (mode: EditorMode) => void;
  tool: ToolId;
  onToolChange: (tool: ToolId) => void;
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
    tool,
    onToolChange,
    canUndo,
    canRedo,
    onUndo,
    onRedo,
    onExport,
    onBackHome,
  } = props;
  const tools = toolsForMode(mode);
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
      <div
        role="toolbar"
        aria-label="Drawing tools"
        style={{
          display: "flex",
          gap: 2,
          padding: "2px",
          background: colors.bgSoft,
          borderRadius: radius.pill,
          marginLeft: spacing.md,
        }}
      >
        {tools.map((t) => (
          <button
            key={t}
            type="button"
            onClick={() => onToolChange(t)}
            aria-pressed={t === tool}
            title={`${TOOL_LABELS[t].label} (${TOOL_LABELS[t].key})`}
            style={toolButton(t === tool)}
          >
            {TOOL_LABELS[t].label}
          </button>
        ))}
      </div>
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
    cursor: "pointer",
  };
}

function toolButton(active: boolean): React.CSSProperties {
  return {
    border: "1px solid transparent",
    background: active ? colors.bg : "transparent",
    color: active ? colors.accent : colors.textMuted,
    borderRadius: radius.pill,
    padding: "4px 10px",
    fontSize: 11,
    fontWeight: active ? 600 : 500,
    cursor: "pointer",
  };
}

import { useTheme } from "../styles/ThemeProvider";
import { colors, font, radius, spacing } from "../styles/tokens";
import type { ToolId } from "../pages/EditorPage";
import { Icon, type IconName } from "./Icon";

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

const TOOL_LABELS: Record<
  ToolId,
  { label: string; key: string; icon: IconName }
> = {
  select: { label: "Select", key: "V", icon: "mouse-pointer" },
  rect: { label: "Rect", key: "R", icon: "square" },
  ellipse: { label: "Ellipse", key: "E", icon: "circle" },
  line: { label: "Line", key: "L", icon: "minus" },
  text: { label: "Text", key: "T", icon: "type" },
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
  const { themeId, toggle: toggleTheme } = useTheme();
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
        <span style={iconRow()}>
          <Icon name="arrow-left" size={14} />
          Home
        </span>
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
            aria-label={TOOL_LABELS[t].label}
            title={`${TOOL_LABELS[t].label} (${TOOL_LABELS[t].key})`}
            style={toolButton(t === tool)}
          >
            <Icon name={TOOL_LABELS[t].icon} size={16} />
          </button>
        ))}
      </div>
      <div style={{ flex: 1 }} />
      <button
        type="button"
        onClick={onUndo}
        disabled={!canUndo}
        aria-label="Undo"
        title="Undo"
        style={pillButton(false, !canUndo)}
      >
        <Icon name="undo" size={14} />
      </button>
      <button
        type="button"
        onClick={onRedo}
        disabled={!canRedo}
        aria-label="Redo"
        title="Redo"
        style={pillButton(false, !canRedo)}
      >
        <Icon name="redo" size={14} />
      </button>
      <button
        type="button"
        onClick={toggleTheme}
        style={pillButton(false)}
        aria-label={
          themeId === "dark"
            ? "Switch to light theme"
            : "Switch to dark theme"
        }
        title={`Theme: ${themeId === "dark" ? "Dark" : "Light"}`}
      >
        <Icon name={themeId === "dark" ? "sun" : "moon"} size={14} />
      </button>
      <button type="button" onClick={onExport} style={pillButton(true)}>
        <span style={iconRow()}>
          <Icon name="download" size={14} />
          Export
        </span>
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
    padding: "4px 8px",
    fontSize: 11,
    fontWeight: active ? 600 : 500,
    cursor: "pointer",
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
  };
}

/// Inline-flex row used by the Home / Export / undo / redo pills to
/// align the icon to the label without leaving an awkward baseline
/// gap. Centralised here so every icon+label pill in the TopBar uses
/// the same vertical-centering rule.
function iconRow(): React.CSSProperties {
  return {
    display: "inline-flex",
    alignItems: "center",
    gap: 6,
  };
}

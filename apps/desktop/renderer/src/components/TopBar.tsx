import { useTheme } from "../styles/ThemeProvider";
import { colors, font, radius, spacing } from "../styles/tokens";
import type { ToolId } from "../pages/EditorPage";
import { useI18n } from "../i18n";
import { formatBinding } from "../shortcuts/registry";
import { useShortcutBindings } from "../shortcuts/useShortcuts";
import { Icon, type IconName } from "./Icon";
import { LanguageSwitcher } from "./LanguageSwitcher";

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
  design: ["select", "rect", "ellipse", "line", "pen", "text"],
  vector: ["select", "rect", "ellipse", "line", "pen"],
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

export const TOOL_LABELS: Record<
  ToolId,
  { label: string; key: string; icon: IconName }
> = {
  select: { label: "Select", key: "V", icon: "mouse-pointer" },
  rect: { label: "Rect", key: "R", icon: "square" },
  ellipse: { label: "Ellipse", key: "E", icon: "circle" },
  line: { label: "Line", key: "L", icon: "minus" },
  pen: { label: "Pen", key: "P", icon: "pen-tool" },
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
  // H1 — discoverability entry points. Optional so existing callers /
  // tests that don't wire them still compile; each button only renders
  // when its handler is supplied.
  /** Open the fuzzy command palette (mouse entry for Cmd/Ctrl+K). */
  onOpenCommandPalette?: () => void;
  /** Open the template picker ("Start from a template"). */
  onOpenTemplates?: () => void;
  /** Open the AI themed-design brief ("Generate with AI"). */
  onOpenAiGenerate?: () => void;
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
    onOpenCommandPalette,
    onOpenTemplates,
    onOpenAiGenerate,
  } = props;
  const tools = toolsForMode(mode);
  const { themeId, toggle: toggleTheme } = useTheme();
  const { t } = useI18n();
  // Live binding so the palette hint reflects any user rebind rather
  // than a hard-coded "⌘K".
  const bindings = useShortcutBindings();
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
        aria-label={t("topbar.aria.backToHome")}
      >
        <span style={ICON_ROW_STYLE}>
          <Icon name="arrow-left" size={14} />
          {t("topbar.home")}
        </span>
      </button>
      <span style={{ fontWeight: 600 }}>{projectName}</span>
      {onOpenCommandPalette ? (
        <button
          type="button"
          onClick={onOpenCommandPalette}
          style={paletteTriggerStyle}
          aria-label={t("topbar.aria.openCommandPalette")}
          title={t("topbar.search.hint")}
        >
          <Icon name="command" size={14} />
          <span>{t("topbar.search")}</span>
          <kbd style={paletteHintStyle}>
            {formatBinding(bindings.openCommandPalette)}
          </kbd>
        </button>
      ) : null}
      <nav
        style={{
          display: "flex",
          gap: spacing.xs,
          marginInlineStart: spacing.md,
        }}
        aria-label={t("topbar.aria.editorMode")}
      >
        {EDITOR_MODES.map(({ mode: m }) => (
          <button
            key={m}
            type="button"
            onClick={() => onModeChange(m)}
            style={modeTab(m === mode)}
            aria-pressed={m === mode}
          >
            {t(`topbar.mode.${m}`)}
          </button>
        ))}
      </nav>
      <div
        role="toolbar"
        aria-label={t("topbar.aria.drawingTools")}
        style={{
          display: "flex",
          gap: 2,
          padding: "2px",
          background: colors.bgSoft,
          borderRadius: radius.pill,
          marginInlineStart: spacing.md,
        }}
      >
        {tools.map((toolId) => {
          const label = t(`topbar.tool.${toolId}`);
          return (
            <button
              key={toolId}
              type="button"
              onClick={() => onToolChange(toolId)}
              aria-pressed={toolId === tool}
              aria-label={label}
              title={t("topbar.tool.title", {
                label,
                key: TOOL_LABELS[toolId].key,
              })}
              style={toolButton(toolId === tool)}
            >
              <Icon name={TOOL_LABELS[toolId].icon} size={16} />
            </button>
          );
        })}
      </div>
      <div style={{ flex: 1 }} />
      {onOpenTemplates ? (
        <button
          type="button"
          onClick={onOpenTemplates}
          style={pillButton(false)}
          aria-label={t("topbar.aria.browseTemplates")}
          title={t("topbar.templates.hint")}
        >
          <span style={ICON_ROW_STYLE}>
            <Icon name="layout" size={14} />
            {t("topbar.templates")}
          </span>
        </button>
      ) : null}
      {onOpenAiGenerate ? (
        <button
          type="button"
          onClick={onOpenAiGenerate}
          style={pillButton(false)}
          aria-label={t("topbar.aria.generateWithAi")}
          title={t("topbar.generate.hint")}
        >
          <span style={ICON_ROW_STYLE}>
            <Icon name="sparkles" size={14} />
            {t("topbar.generate")}
          </span>
        </button>
      ) : null}
      <button
        type="button"
        onClick={onUndo}
        disabled={!canUndo}
        aria-label={t("topbar.aria.undo")}
        title={t("topbar.aria.undo")}
        style={pillButton(false, !canUndo)}
      >
        <Icon name="undo" size={14} />
      </button>
      <button
        type="button"
        onClick={onRedo}
        disabled={!canRedo}
        aria-label={t("topbar.aria.redo")}
        title={t("topbar.aria.redo")}
        style={pillButton(false, !canRedo)}
      >
        <Icon name="redo" size={14} />
      </button>
      <LanguageSwitcher />
      <button
        type="button"
        onClick={toggleTheme}
        style={pillButton(false)}
        aria-label={
          themeId === "dark"
            ? t("topbar.aria.switchToLight")
            : t("topbar.aria.switchToDark")
        }
        title={themeId === "dark" ? t("topbar.theme.dark") : t("topbar.theme.light")}
      >
        <Icon name={themeId === "dark" ? "sun" : "moon"} size={14} />
      </button>
      <button type="button" onClick={onExport} style={pillButton(true)}>
        <span style={ICON_ROW_STYLE}>
          <Icon name="download" size={14} />
          {t("topbar.export")}
        </span>
      </button>
    </header>
  );
}

const paletteTriggerStyle: React.CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 6,
  marginInlineStart: spacing.sm,
  border: `1px solid ${colors.border}`,
  background: colors.bgSoft,
  color: colors.textMuted,
  borderRadius: radius.pill,
  padding: "5px 10px",
  fontSize: 12,
  fontWeight: 500,
  cursor: "pointer",
};

const paletteHintStyle: React.CSSProperties = {
  fontSize: 10,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  padding: "0 5px",
  color: colors.textMuted,
};

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
///
/// Hoisted to a module-level constant (rather than the
/// `pillButton`/`modeTab`/`toolButton` factory-function pattern used
/// elsewhere in this file) because it takes zero arguments and the
/// returned object is identical for every call site. Allocating a
/// fresh style object on every render would defeat React's reference
/// equality on the `style` prop and force the underlying DOM node to
/// rewrite inline styles on every parent re-render even when nothing
/// changed. The factory functions above genuinely need to remain
/// functions because their output varies by argument (`primary`,
/// `active`, `disabled`).
const ICON_ROW_STYLE: React.CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: 6,
};

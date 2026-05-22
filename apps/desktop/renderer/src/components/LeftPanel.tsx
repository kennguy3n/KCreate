import { useState } from "react";

import type {
  ArtboardInfo,
  ComponentInfo,
  NodeInfo,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";
import { ArtboardPanel } from "./ArtboardPanel";
import { ComponentPanel } from "./ComponentPanel";

export type LeftPanelTab = "pages" | "artboards" | "layers" | "assets";

export interface LeftPanelProps {
  nodes: NodeInfo[];
  selectedId: string | null;
  selectedIds?: string[];
  onSelect: (id: string | null) => void;
  onToggleVisibility?: (id: string, visible: boolean) => void;
  onToggleLocked?: (id: string, locked: boolean) => void;
  onRename?: (id: string, name: string) => void;
  onDelete?: (id: string) => void;

  // Artboard-tab inputs. Optional so existing callers that haven't
  // wired the artboard bridge yet keep working (the tab just renders
  // an empty state).
  artboards?: ArtboardInfo[];
  onRequestCreateArtboard?: () => void;
  onFocusArtboard?: (artboard: ArtboardInfo) => void;
  onRenameArtboard?: (id: string, name: string) => void;
  onDuplicateArtboard?: (id: string) => void;
  onResizeArtboard?: (id: string, width: number, height: number) => void;
  onDeleteArtboard?: (id: string) => void;

  // Components-section inputs. Optional so existing callers that
  // haven't wired the component bridge yet keep working (the assets
  // tab just falls back to the empty hint).
  components?: ComponentInfo[];
  onComponentCreateFromSelection?: (name: string) => void;
  onComponentInstantiate?: (componentId: string) => void;
  onComponentAddVariant?: (componentId: string, name: string) => void;
  onComponentSwitchVariant?: (nodeId: string, variantId: string) => void;
  onComponentDetach?: (nodeId: string) => void;
}

export function LeftPanel({
  nodes,
  selectedId,
  selectedIds,
  onSelect,
  onToggleVisibility,
  onToggleLocked,
  onRename,
  onDelete,
  artboards,
  onRequestCreateArtboard,
  onFocusArtboard,
  onRenameArtboard,
  onDuplicateArtboard,
  onResizeArtboard,
  onDeleteArtboard,
  components,
  onComponentCreateFromSelection,
  onComponentInstantiate,
  onComponentAddVariant,
  onComponentSwitchVariant,
  onComponentDetach,
}: LeftPanelProps): JSX.Element {
  const [tab, setTab] = useState<LeftPanelTab>("layers");
  return (
    <aside
      style={{
        width: 260,
        background: colors.bg,
        borderRight: `1px solid ${colors.border}`,
        display: "flex",
        flexDirection: "column",
      }}
    >
      <PanelTabs
        tabs={[
          { id: "pages", label: "Pages" },
          { id: "artboards", label: "Artboards" },
          { id: "layers", label: "Layers" },
          { id: "assets", label: "Assets" },
        ]}
        active={tab}
        onChange={setTab}
      />
      <div
        style={{
          flex: 1,
          overflowY: "auto",
          padding: spacing.sm,
        }}
      >
        {tab === "pages" ? (
          <NodeList
            nodes={nodes.filter((n) => n.nodeType === "Page")}
            selectedId={selectedId}
            onSelect={onSelect}
            onToggleVisibility={onToggleVisibility}
            onToggleLocked={onToggleLocked}
            onRename={onRename}
            onDelete={onDelete}
            emptyHint="No pages yet."
          />
        ) : null}
        {tab === "artboards" ? (
          <ArtboardPanel
            artboards={artboards ?? []}
            selectedId={selectedId}
            onRequestCreate={onRequestCreateArtboard ?? noop}
            onFocusArtboard={onFocusArtboard ?? noopArg}
            onRenameArtboard={onRenameArtboard ?? noopRename}
            onDuplicateArtboard={onDuplicateArtboard ?? noopArg}
            onResizeArtboard={onResizeArtboard ?? noopResize}
            onDeleteArtboard={onDeleteArtboard ?? noopArg}
          />
        ) : null}
        {tab === "layers" ? (
          <NodeList
            nodes={nodes}
            selectedId={selectedId}
            onSelect={onSelect}
            onToggleVisibility={onToggleVisibility}
            onToggleLocked={onToggleLocked}
            onRename={onRename}
            onDelete={onDelete}
            emptyHint="No layers yet."
            showHierarchy
          />
        ) : null}
        {tab === "assets" ? (
          components !== undefined ? (
            <ComponentPanel
              components={components}
              selectedNodeIds={
                selectedIds ?? (selectedId ? [selectedId] : [])
              }
              selectedNode={
                selectedId
                  ? (nodes.find((n) => n.id === selectedId) ?? null)
                  : null
              }
              onCreateFromSelection={
                onComponentCreateFromSelection ?? noopName
              }
              onInstantiate={onComponentInstantiate ?? noopArg}
              onAddVariant={onComponentAddVariant ?? noopRename}
              onSwitchVariant={onComponentSwitchVariant ?? noopRename}
              onDetach={onComponentDetach ?? noopArg}
            />
          ) : (
            <EmptyHint>
              Drop or import images, fonts, and palettes here.
            </EmptyHint>
          )
        ) : null}
      </div>
    </aside>
  );
}

// Empty fallbacks so the panel doesn't need to null-check on every
// click path. Hosts that don't wire the artboard bridge just see a
// no-op `+ New artboard` button and an empty list.
const noop = (): void => undefined;
const noopArg = (_: unknown): void => undefined;
const noopName = (_: string): void => undefined;
const noopRename = (_: string, __: string): void => undefined;
const noopResize = (_: string, __: number, ___: number): void => undefined;

function PanelTabs<T extends string>({
  tabs,
  active,
  onChange,
}: {
  tabs: ReadonlyArray<{ id: T; label: string }>;
  active: T;
  onChange: (id: T) => void;
}): JSX.Element {
  return (
    <div
      style={{
        display: "flex",
        gap: 2,
        padding: `${spacing.sm}px ${spacing.sm}px 0`,
      }}
      role="tablist"
    >
      {tabs.map((t) => (
        <button
          key={t.id}
          type="button"
          role="tab"
          aria-selected={active === t.id}
          onClick={() => onChange(t.id)}
          style={{
            flex: 1,
            padding: "6px 8px",
            fontSize: 12,
            fontWeight: 500,
            background: "transparent",
            border: "none",
            color: active === t.id ? colors.accent : colors.textMuted,
            borderBottom: `2px solid ${
              active === t.id ? colors.accent : "transparent"
            }`,
            cursor: "pointer",
          }}
        >
          {t.label}
        </button>
      ))}
    </div>
  );
}

function NodeList({
  nodes,
  selectedId,
  onSelect,
  onToggleVisibility,
  onToggleLocked,
  onRename,
  onDelete,
  emptyHint,
  showHierarchy = false,
}: {
  nodes: NodeInfo[];
  selectedId: string | null;
  onSelect: (id: string | null) => void;
  onToggleVisibility?: (id: string, visible: boolean) => void;
  onToggleLocked?: (id: string, locked: boolean) => void;
  onRename?: (id: string, name: string) => void;
  onDelete?: (id: string) => void;
  emptyHint: string;
  showHierarchy?: boolean;
}): JSX.Element {
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [draftName, setDraftName] = useState("");

  if (nodes.length === 0) {
    return <EmptyHint>{emptyHint}</EmptyHint>;
  }
  // Build depth map by walking parent_id chain.
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const depthOf = (n: NodeInfo): number => {
    let d = 0;
    let cur: NodeInfo | undefined = n;
    while (cur?.parentId) {
      const parent = byId.get(cur.parentId);
      if (!parent) break;
      d += 1;
      cur = parent;
    }
    return d;
  };

  const commitRename = (id: string): void => {
    if (draftName.trim().length > 0) onRename?.(id, draftName.trim());
    setRenamingId(null);
    setDraftName("");
  };

  return (
    <ul
      style={{
        listStyle: "none",
        margin: 0,
        padding: 0,
        display: "flex",
        flexDirection: "column",
        gap: 1,
      }}
    >
      {nodes.map((n) => {
        const depth = showHierarchy ? depthOf(n) : 0;
        const selected = n.id === selectedId;
        const isRenaming = renamingId === n.id;
        return (
          <li key={n.id}>
            <div
              style={{
                display: "flex",
                alignItems: "center",
                gap: 4,
                padding: `4px 6px 4px ${6 + depth * 12}px`,
                background: selected ? colors.bgSoft : "transparent",
                borderRadius: radius.card / 2,
                fontSize: 12,
                cursor: "pointer",
              }}
              onClick={() => {
                if (!isRenaming) onSelect(selected ? null : n.id);
              }}
              onDoubleClick={(e) => {
                e.stopPropagation();
                setRenamingId(n.id);
                setDraftName(n.name);
              }}
            >
              <button
                type="button"
                aria-label={n.visible ? "Hide layer" : "Show layer"}
                title={n.visible ? "Visible" : "Hidden"}
                onClick={(e) => {
                  e.stopPropagation();
                  onToggleVisibility?.(n.id, !n.visible);
                }}
                style={iconButton(n.visible)}
              >
                {n.visible ? "●" : "○"}
              </button>
              <button
                type="button"
                aria-label={n.locked ? "Unlock layer" : "Lock layer"}
                title={n.locked ? "Locked" : "Unlocked"}
                onClick={(e) => {
                  e.stopPropagation();
                  onToggleLocked?.(n.id, !n.locked);
                }}
                style={iconButton(n.locked)}
              >
                {n.locked ? "⌧" : "⌬"}
              </button>
              <span
                style={{
                  fontSize: 10,
                  color: colors.textMuted,
                  textTransform: "uppercase",
                  letterSpacing: 0.4,
                  width: 14,
                  textAlign: "center",
                }}
              >
                {nodeTypeAbbrev(n.nodeType)}
              </span>
              {isRenaming ? (
                <input
                  autoFocus
                  value={draftName}
                  onChange={(e) => setDraftName(e.target.value)}
                  onBlur={() => commitRename(n.id)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") commitRename(n.id);
                    if (e.key === "Escape") {
                      setRenamingId(null);
                      setDraftName("");
                    }
                  }}
                  onClick={(e) => e.stopPropagation()}
                  style={{
                    flex: 1,
                    background: colors.bg,
                    color: colors.text,
                    border: `1px solid ${colors.accent}`,
                    borderRadius: 3,
                    padding: "1px 4px",
                    fontSize: 12,
                  }}
                />
              ) : (
                <span
                  style={{
                    flex: 1,
                    color: selected ? colors.accent : colors.text,
                    fontWeight: selected ? 600 : 400,
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    whiteSpace: "nowrap",
                  }}
                >
                  {n.name}
                </span>
              )}
              {onDelete ? (
                <button
                  type="button"
                  aria-label="Delete layer"
                  title="Delete"
                  onClick={(e) => {
                    e.stopPropagation();
                    onDelete(n.id);
                  }}
                  style={iconButton(false)}
                >
                  ×
                </button>
              ) : null}
            </div>
          </li>
        );
      })}
    </ul>
  );
}

function iconButton(active: boolean): React.CSSProperties {
  return {
    width: 18,
    height: 18,
    fontSize: 12,
    lineHeight: "16px",
    background: "transparent",
    border: "none",
    color: active ? colors.text : colors.textMuted,
    cursor: "pointer",
    padding: 0,
    borderRadius: 3,
  };
}

function nodeTypeAbbrev(t: string): string {
  switch (t) {
    case "Page":
      return "P";
    case "Artboard":
      return "A";
    case "GroupLayer":
      return "G";
    case "VectorLayer":
      return "V";
    case "RasterLayer":
      return "R";
    case "TextLayer":
      return "T";
    case "ComponentLayer":
      return "C";
    case "LayoutFrame":
      return "L";
    default:
      return "?";
  }
}

function EmptyHint({
  children,
}: {
  children: React.ReactNode;
}): JSX.Element {
  return (
    <div
      style={{
        padding: spacing.md,
        fontSize: 12,
        color: colors.textMuted,
        lineHeight: 1.5,
      }}
    >
      {children}
    </div>
  );
}

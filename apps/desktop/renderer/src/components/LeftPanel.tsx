import { useState } from "react";

import type {
  ArtboardInfo,
  ComponentInfo,
  NodeInfo,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";
import { ArtboardPanel } from "./ArtboardPanel";
import { BrandKitEditor } from "./BrandKitEditor";
import { BrandVersionPanel } from "./BrandVersionPanel";
import { ComponentPanel } from "./ComponentPanel";
import { DesignTokenEditor } from "./DesignTokenEditor";

export type LeftPanelTab =
  | "pages"
  | "artboards"
  | "layers"
  | "assets"
  | "tokens"
  | "brand";

// Phase 6 Tasks 27-28 — layer colour palette. Canonical lowercase
// keys match `kcreate_bridge::document::canonicalise_layer_color`
// (whitespace + case-folded), so the Rust side accepts them
// verbatim. The swatch values are the actual hex codes the LayerPanel
// renders on the tag pill.
export const LAYER_COLOR_PALETTE: ReadonlyArray<{ key: string; swatch: string }> = [
  { key: "red", swatch: "#EF4444" },
  { key: "orange", swatch: "#F97316" },
  { key: "yellow", swatch: "#F59E0B" },
  { key: "green", swatch: "#10B981" },
  { key: "blue", swatch: "#3B82F6" },
  { key: "violet", swatch: "#7C3AED" },
  { key: "gray", swatch: "#6B7280" },
];

const LAYER_COLOR_LOOKUP: Record<string, string> = Object.fromEntries(
  LAYER_COLOR_PALETTE.map((p) => [p.key, p.swatch]),
);

export interface LeftPanelProps {
  nodes: NodeInfo[];
  selectedId: string | null;
  selectedIds?: string[];
  onSelect: (id: string | null) => void;
  onSelectMany?: (ids: string[]) => void;
  onToggleVisibility?: (id: string, visible: boolean) => void;
  onToggleLocked?: (id: string, locked: boolean) => void;
  onRename?: (id: string, name: string) => void;
  onDelete?: (id: string) => void;
  /**
   * Phase 6 Tasks 27-28 — install (or clear, when `color` is null)
   * a layer-colour tag on the given node. The host wires this to
   * `window.kcreate.setLayerColor` so the colour swatch participates
   * in undo/redo via the `layer_color_set` op.
   */
  onSetLayerColor?: (id: string, color: string | null) => void;

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

  // Design-system inputs (tokens + brand kits). Optional so callers
  // that haven't wired these bridges yet keep working; both tabs
  // render their own empty state when not present.
  onDesignSystemStatus?: (msg: string | null) => void;
}

export function LeftPanel({
  nodes,
  selectedId,
  selectedIds,
  onSelect,
  onSelectMany,
  onToggleVisibility,
  onToggleLocked,
  onRename,
  onDelete,
  onSetLayerColor,
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
  onDesignSystemStatus,
}: LeftPanelProps): JSX.Element {
  const [tab, setTab] = useState<LeftPanelTab>("layers");
  // Phase 6 Tasks 27-28 — case-insensitive name filter for the
  // layers tab. Empty string means "show everything". Kept local to
  // LeftPanel so other tabs aren't filtered by the layer query.
  const [layerQuery, setLayerQuery] = useState("");
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
          { id: "tokens", label: "Tokens" },
          { id: "brand", label: "Brand" },
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
          <LayerTabContent
            nodes={nodes}
            selectedId={selectedId}
            query={layerQuery}
            onQueryChange={setLayerQuery}
            onSelect={onSelect}
            onSelectMany={onSelectMany ?? noopMany}
            onToggleVisibility={onToggleVisibility}
            onToggleLocked={onToggleLocked}
            onRename={onRename}
            onDelete={onDelete}
            onSetLayerColor={onSetLayerColor ?? noopLayerColor}
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
        {tab === "tokens" ? (
          <DesignTokenEditor
            selectedNodeId={selectedId}
            onStatus={onDesignSystemStatus ?? noopStatus}
          />
        ) : null}
        {tab === "brand" ? (
          <div
            style={{ display: "flex", flexDirection: "column", gap: spacing.md }}
          >
            <BrandKitEditor onStatus={onDesignSystemStatus ?? noopStatus} />
            {/* Phase 8 Task 16 — brand-kit version history. Lives
                under the Brand tab so the user can save / restore /
                diff versions next to the kit being edited. */}
            <BrandVersionPanel
              onStatus={onDesignSystemStatus ?? noopStatus}
            />
          </div>
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
const noopStatus = (_: string | null): void => undefined;
const noopMany = (_: string[]): void => undefined;
const noopLayerColor = (_: string, __: string | null): void => undefined;

/// Phase 6 Tasks 27-28 — layers-tab content: search filter +
/// "select all of type" controls + the NodeList itself. Pulled out
/// so the layers tab can hold its own state (the search query, the
/// type filter, the colour-tag popover) without re-rendering every
/// other tab.
function LayerTabContent({
  nodes,
  selectedId,
  query,
  onQueryChange,
  onSelect,
  onSelectMany,
  onToggleVisibility,
  onToggleLocked,
  onRename,
  onDelete,
  onSetLayerColor,
}: {
  nodes: NodeInfo[];
  selectedId: string | null;
  query: string;
  onQueryChange: (q: string) => void;
  onSelect: (id: string | null) => void;
  onSelectMany: (ids: string[]) => void;
  onToggleVisibility?: (id: string, visible: boolean) => void;
  onToggleLocked?: (id: string, locked: boolean) => void;
  onRename?: (id: string, name: string) => void;
  onDelete?: (id: string) => void;
  onSetLayerColor: (id: string, color: string | null) => void;
}): JSX.Element {
  // The query matches case-insensitively against the node name AND
  // its installed colour-tag key, so "red" surfaces every red-tagged
  // layer regardless of name.
  const needle = query.trim().toLowerCase();
  const filtered = needle
    ? nodes.filter((n) => {
        if (n.name.toLowerCase().includes(needle)) return true;
        const tag = layerColorOf(n);
        return tag !== null && tag.toLowerCase().includes(needle);
      })
    : nodes;

  // Distinct node types present in the (unfiltered) tree, in stable
  // order, for the "select all of type" dropdown.
  const types: string[] = [];
  for (const n of nodes) {
    if (!types.includes(n.nodeType)) types.push(n.nodeType);
  }

  const handleSelectAllOfType = (t: string): void => {
    const ids = nodes.filter((n) => n.nodeType === t).map((n) => n.id);
    if (ids.length > 0) onSelectMany(ids);
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: spacing.sm }}>
      <div
        style={{
          display: "flex",
          gap: 4,
          alignItems: "center",
          padding: `0 ${spacing.xs}px`,
        }}
      >
        <input
          type="search"
          value={query}
          onChange={(e) => onQueryChange(e.target.value)}
          placeholder="Filter layers (name or colour)"
          aria-label="Filter layers"
          style={{
            flex: 1,
            background: colors.bg,
            color: colors.text,
            border: `1px solid ${colors.border}`,
            borderRadius: radius.card / 2,
            padding: "4px 8px",
            fontSize: 12,
          }}
        />
        {query.length > 0 ? (
          <button
            type="button"
            aria-label="Clear filter"
            title="Clear filter"
            onClick={() => onQueryChange("")}
            style={iconButton(true)}
          >
            ×
          </button>
        ) : null}
      </div>
      {types.length > 1 ? (
        <div
          style={{
            display: "flex",
            flexWrap: "wrap",
            gap: 4,
            padding: `0 ${spacing.xs}px`,
          }}
        >
          {types.map((t) => (
            <button
              key={t}
              type="button"
              onClick={() => handleSelectAllOfType(t)}
              title={`Select every ${t}`}
              style={{
                fontSize: 10,
                padding: "2px 6px",
                background: colors.bgSoft,
                color: colors.textMuted,
                border: `1px solid ${colors.border}`,
                borderRadius: 3,
                cursor: "pointer",
              }}
            >
              All {t}
            </button>
          ))}
        </div>
      ) : null}
      <NodeList
        nodes={filtered}
        selectedId={selectedId}
        onSelect={onSelect}
        onToggleVisibility={onToggleVisibility}
        onToggleLocked={onToggleLocked}
        onRename={onRename}
        onDelete={onDelete}
        onSetLayerColor={onSetLayerColor}
        emptyHint={
          needle.length > 0
            ? `No layers matching '${query}'.`
            : "No layers yet."
        }
        showHierarchy={needle.length === 0}
      />
    </div>
  );
}

/// Read the canonical colour-tag key off `Node::metadata.layerColor`.
/// Returns `null` when the tag is missing or the metadata field
/// isn't a string (defensive; the Rust side only writes strings).
function layerColorOf(n: NodeInfo): string | null {
  const raw = n.metadata?.["layerColor"];
  if (typeof raw === "string" && raw.length > 0) return raw;
  return null;
}

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
  onSetLayerColor,
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
  onSetLayerColor?: (id: string, color: string | null) => void;
  emptyHint: string;
  showHierarchy?: boolean;
}): JSX.Element {
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [draftName, setDraftName] = useState("");
  // Which row is currently showing its colour-tag popover. Only one
  // popover at a time so a click outside the row dismisses it.
  const [colorPopoverId, setColorPopoverId] = useState<string | null>(null);

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
              {onSetLayerColor ? (
                <ColorTagControl
                  current={layerColorOf(n)}
                  open={colorPopoverId === n.id}
                  onToggle={() =>
                    setColorPopoverId((id) => (id === n.id ? null : n.id))
                  }
                  onPick={(key) => {
                    onSetLayerColor(n.id, key);
                    setColorPopoverId(null);
                  }}
                />
              ) : null}
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

/// Compact dot-button + popover for installing / clearing a
/// layer-colour tag. The dot reads the current tag; clicking it
/// toggles a small palette popover. Picking a swatch calls
/// `onPick(key)`; picking the "clear" tile calls `onPick(null)`.
function ColorTagControl({
  current,
  open,
  onToggle,
  onPick,
}: {
  current: string | null;
  open: boolean;
  onToggle: () => void;
  onPick: (key: string | null) => void;
}): JSX.Element {
  const swatch =
    current && LAYER_COLOR_LOOKUP[current]
      ? LAYER_COLOR_LOOKUP[current]
      : null;
  return (
    <div style={{ position: "relative" }} onClick={(e) => e.stopPropagation()}>
      <button
        type="button"
        aria-label={current ? `Layer tag: ${current}` : "Tag layer"}
        title={current ? `Tag: ${current} (click to change)` : "Tag layer"}
        onClick={onToggle}
        style={{
          width: 14,
          height: 14,
          borderRadius: "50%",
          border: `1px solid ${swatch ? swatch : colors.border}`,
          background: swatch ?? "transparent",
          cursor: "pointer",
          padding: 0,
        }}
      />
      {open ? (
        <div
          role="menu"
          style={{
            position: "absolute",
            top: 18,
            right: 0,
            zIndex: 10,
            display: "flex",
            gap: 4,
            padding: 6,
            background: colors.bgSoft,
            border: `1px solid ${colors.border}`,
            borderRadius: 4,
            boxShadow: "0 4px 12px rgba(0, 0, 0, 0.2)",
          }}
        >
          {LAYER_COLOR_PALETTE.map((p) => (
            <button
              key={p.key}
              type="button"
              aria-label={`Tag ${p.key}`}
              title={p.key}
              onClick={() => onPick(p.key)}
              style={{
                width: 14,
                height: 14,
                borderRadius: "50%",
                border:
                  current === p.key
                    ? `2px solid ${colors.text}`
                    : `1px solid ${colors.border}`,
                background: p.swatch,
                cursor: "pointer",
                padding: 0,
              }}
            />
          ))}
          <button
            type="button"
            aria-label="Clear tag"
            title="Clear tag"
            onClick={() => onPick(null)}
            style={{
              width: 14,
              height: 14,
              borderRadius: "50%",
              border: `1px dashed ${colors.border}`,
              background: "transparent",
              color: colors.textMuted,
              fontSize: 9,
              lineHeight: "12px",
              cursor: "pointer",
              padding: 0,
            }}
          >
            ×
          </button>
        </div>
      ) : null}
    </div>
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

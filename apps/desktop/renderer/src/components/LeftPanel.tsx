import { useCallback, useEffect, useState } from "react";

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
import {
  ContextMenu,
  MenuDivider,
  MenuItem,
  MenuSubheading,
} from "./ContextMenu";
import { DesignTokenEditor } from "./DesignTokenEditor";
import { Icon, type IconName } from "./Icon";

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
   * Phase D Polish — duplicate a single layer-tree node. Wired by
   * the host (`EditorPage`) to the existing copy+paste flow so the
   * undo/redo log and clipboard semantics stay consistent with the
   * keyboard shortcut path. Optional so existing callers (e.g. the
   * test harness) keep compiling; the layer-tree context menu hides
   * the "Duplicate" item when this isn't wired.
   */
  onDuplicateNode?: (id: string) => void;
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
  onMagicResize?: (id: string) => void;
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
  onDuplicateNode,
  onSetLayerColor,
  artboards,
  onRequestCreateArtboard,
  onFocusArtboard,
  onRenameArtboard,
  onDuplicateArtboard,
  onResizeArtboard,
  onMagicResize,
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
          // Icons paired with each tab mirror the RightPanel pattern
          // (see `mkTab` + `BASE_TABS` in `RightPanel.tsx`) so the two
          // side panels read as the same control surface. Picked by
          // semantic match: layers ↔ stacked rectangles, artboards ↔
          // boxed frame, brand ↔ palette, etc. The leading icon is
          // 14px (inline) so the strip stays narrow enough for a 260px
          // panel without wrapping the labels.
          { id: "pages", label: "Pages", icon: "file-text" },
          { id: "artboards", label: "Artboards", icon: "frame" },
          { id: "layers", label: "Layers", icon: "layers" },
          { id: "assets", label: "Assets", icon: "package" },
          { id: "tokens", label: "Tokens", icon: "variable" },
          { id: "brand", label: "Brand", icon: "palette" },
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
            onMagicResize={onMagicResize ?? noopArg}
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
            onDuplicateNode={onDuplicateNode}
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
  onDuplicateNode,
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
  onDuplicateNode?: (id: string) => void;
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
            <Icon name="x" size={12} />
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
        onDuplicateNode={onDuplicateNode}
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
  /// Each tab carries an optional Lucide `icon` rendered before the
  /// label. Optional (not required) so existing callers — and any
  /// future ones that need a label-only tab strip — keep compiling
  /// without a sentinel value.
  tabs: ReadonlyArray<{ id: T; label: string; icon?: IconName }>;
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
            // `flex: 1` distributes the strip width evenly; the
            // inline-flex inner row aligns icon + label without
            // disturbing that outer distribution.
            flex: 1,
            display: "inline-flex",
            alignItems: "center",
            justifyContent: "center",
            gap: 4,
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
          {t.icon ? <Icon name={t.icon} size={14} /> : null}
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
  onDuplicateNode,
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
  onDuplicateNode?: (id: string) => void;
  onSetLayerColor?: (id: string, color: string | null) => void;
  emptyHint: string;
  showHierarchy?: boolean;
}): JSX.Element {
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [draftName, setDraftName] = useState("");
  // Which row is currently showing its colour-tag popover. Only one
  // popover at a time so a click outside the row dismisses it.
  const [colorPopoverId, setColorPopoverId] = useState<string | null>(null);
  // Phase D — right-click context menu state for the layer tree.
  const [ctxMenu, setCtxMenu] = useState<{
    nodeId: string;
    x: number;
    y: number;
  } | null>(null);

  // Phase D — clear `ctxMenu` if the targeted row drops out of the
  // visible `nodes` list (e.g. the user opens the menu then types in
  // the search filter, or the bridge reports a tree refresh that
  // removed the node). Otherwise the menu silently disappears in the
  // render path (`nodes.find(...) === undefined` short-circuits the
  // IIFE below) but the `ctxMenu` state lingers, so a subsequent
  // re-add of the same id would resurrect the menu at the old (x, y).
  // The destructured `ctxMenu?.nodeId` is the only reactive dep we
  // care about — re-running on every `nodes` identity change would
  // also work but would fire on every tree refresh, including ones
  // where the targeted row is still present.
  useEffect(() => {
    if (!ctxMenu) return;
    if (!nodes.some((n) => n.id === ctxMenu.nodeId)) {
      setCtxMenu(null);
    }
  }, [ctxMenu, nodes]);

  // Phase D — Devin Review ANALYSIS_0004 on `ab2bb5f`: pass a stable
  // `closeCtxMenu` identity to `<ContextMenu onDismiss>` (and reuse it
  // from every item that just wants to close the menu) so the Escape
  // and outside-click `useEffect` hooks inside `ContextMenu` don't
  // detach + reattach their capture-phase listeners on every parent
  // re-render while the menu is open. The empty dep array is safe —
  // `setCtxMenu` is React-guaranteed-stable and we always close to
  // `null`, not a derived value.
  const closeCtxMenu = useCallback((): void => setCtxMenu(null), []);

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
    <>
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
              onContextMenu={(e) => {
                e.preventDefault();
                e.stopPropagation();
                setCtxMenu({ nodeId: n.id, x: e.clientX, y: e.clientY });
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
                <Icon
                  name={n.visible ? "eye" : "eye-off"}
                  size={14}
                />
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
                <Icon
                  name={n.locked ? "lock" : "unlock"}
                  size={14}
                />
              </button>
              <span
                aria-label={`${n.nodeType} layer`}
                title={n.nodeType}
                style={{
                  color: colors.textMuted,
                  width: 14,
                  display: "inline-flex",
                  alignItems: "center",
                  justifyContent: "center",
                }}
              >
                <Icon name={nodeTypeIcon(n.nodeType)} size={12} />
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
                  <Icon name="trash-2" size={14} />
                </button>
              ) : null}
            </div>
          </li>
        );
      })}
    </ul>
    {ctxMenu ? (() => {
      const target = nodes.find((n) => n.id === ctxMenu.nodeId);
      if (!target) return null;
      return (
        <ContextMenu
          x={ctxMenu.x}
          y={ctxMenu.y}
          onDismiss={closeCtxMenu}
          ariaLabel="Layer actions"
        >
          <MenuItem
            label="Rename"
            data-testid="ctx-rename"
            onClick={() => {
              setRenamingId(target.id);
              setDraftName(target.name);
              closeCtxMenu();
            }}
          />
          {onDuplicateNode ? (
            <MenuItem
              label="Duplicate"
              data-testid="ctx-duplicate"
              shortcut="⌘C ⌘V"
              onClick={() => {
                onDuplicateNode(target.id);
                closeCtxMenu();
              }}
            />
          ) : null}
          <MenuDivider />
          <MenuItem
            label={target.visible ? "Hide" : "Show"}
            data-testid="ctx-visibility"
            onClick={() => {
              onToggleVisibility?.(target.id, !target.visible);
              closeCtxMenu();
            }}
          />
          <MenuItem
            label={target.locked ? "Unlock" : "Lock"}
            data-testid="ctx-lock"
            onClick={() => {
              onToggleLocked?.(target.id, !target.locked);
              closeCtxMenu();
            }}
          />
          {onSetLayerColor ? (
            <>
              <MenuDivider />
              <MenuSubheading label="Layer color" />
              {LAYER_COLOR_PALETTE.map((p) => (
                <MenuItem
                  key={p.key}
                  label={p.key.charAt(0).toUpperCase() + p.key.slice(1)}
                  onClick={() => {
                    onSetLayerColor(target.id, p.key);
                    closeCtxMenu();
                  }}
                />
              ))}
              {layerColorOf(target) !== null ? (
                <MenuItem
                  label="Clear color"
                  onClick={() => {
                    onSetLayerColor(target.id, null);
                    closeCtxMenu();
                  }}
                />
              ) : null}
            </>
          ) : null}
          {/*
            Phase D — Devin Review ANALYSIS_0003 on `ab2bb5f`: gate the
            divider on `onDelete` so a caller that omits the prop (the
            interface declares it optional) doesn't render a stranded
            trailing separator under whatever section preceded it.
          */}
          {onDelete ? (
            <>
              <MenuDivider />
              <MenuItem
                label="Delete"
                danger
                data-testid="ctx-delete"
                shortcut="Del"
                onClick={() => {
                  onDelete(target.id);
                  closeCtxMenu();
                }}
              />
            </>
          ) : null}
        </ContextMenu>
      );
    })() : null}
    </>
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

/// Pick a lucide icon that visually summarises a node type for the
/// layer-tree leaf rows. Previously each row showed a single ASCII
/// letter (`P`, `A`, `G`, …), which both collided with the actual
/// layer name and required users to memorise the legend. Icons fit
/// the same 14 px fixed-width column while making the type readable
/// at a glance.
function nodeTypeIcon(t: string): IconName {
  switch (t) {
    case "Page":
      return "file-text";
    case "Artboard":
      return "square";
    case "GroupLayer":
      return "layers";
    case "VectorLayer":
      return "pen-tool";
    case "RasterLayer":
      return "image";
    case "TextLayer":
      return "type";
    case "ComponentLayer":
      return "package";
    case "LayoutFrame":
      return "layout";
    default:
      return "file-text";
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

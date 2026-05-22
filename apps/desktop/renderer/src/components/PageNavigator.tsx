// PageNavigator — Phase 2, Block B, Task 10.
//
// Vertical thumbnail strip of every page in the document. Drives the
// Layout-Studio workflow:
//   - Click a page card to focus it (selects the page node so the
//     canvas pans/zooms to its bounds).
//   - Drag a page card to reorder. Drop position is indicated by a
//     1px line between cards; on drop we call
//     `layoutStudio.reparentNode(pageId, null, dropIndex)`.
//   - Right-click for context-menu actions (Duplicate, Delete,
//     Set page size, Apply master page).
//   - "Add Page" button at the bottom opens an inline size /
//     orientation picker.
//
// Master pages (flagged via `metadata.is_master`) render in a separate
// "Masters" section above the content pages with a dashed border so
// the user can tell at a glance which pages are templates vs. real
// canvas pages.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type {
  MasterPageInfo,
  NodeInfo,
  PageLayout,
  PageOrientation,
  PageSize,
  PageSizeId,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface PageNavigatorProps {
  /** Full document tree — we filter to `nodeType === "Page"` ourselves. */
  nodes: NodeInfo[];
  /** Currently selected page id (drives the highlighted card). */
  selectedPageId: string | null;
  /** Focus a page (select the node so the canvas pans/zooms to it). */
  onSelectPage: (pageId: string) => void;
  /** Status messages for the status bar. */
  onStatus?: (msg: string | null) => void;
  /** Fired after every mutation so the host can refresh the tree. */
  onChanged?: () => void;
  /**
   * Opens the host-owned TemplatePicker modal. When omitted, the
   * "New from Template" button hides. Lets the host decide whether
   * to support template instantiation in this surface.
   */
  onNewFromTemplate?: () => void;
}

type PageCardKind = "master" | "content";

interface PageCard {
  id: string;
  name: string;
  kind: PageCardKind;
  /** Page size label ("A4 portrait", "16:9", "Custom 800x600", …). */
  sizeLabel: string;
  /** Resolved bounds for the thumbnail aspect ratio. */
  aspect: number;
}

const COMMON_SIZES: ReadonlyArray<{
  id: PageSizeId;
  label: string;
  /** Approximate aspect ratio (width / height) at portrait orientation. */
  aspectPortrait: number;
}> = [
  { id: "a4", label: "A4", aspectPortrait: 210 / 297 },
  { id: "a3", label: "A3", aspectPortrait: 297 / 420 },
  { id: "a5", label: "A5", aspectPortrait: 148 / 210 },
  { id: "letter", label: "US Letter", aspectPortrait: 8.5 / 11 },
  { id: "legal", label: "US Legal", aspectPortrait: 8.5 / 14 },
  { id: "tabloid", label: "Tabloid", aspectPortrait: 11 / 17 },
  { id: "presentation_16x9", label: "16:9 slide", aspectPortrait: 9 / 16 },
  { id: "presentation_4x3", label: "4:3 slide", aspectPortrait: 3 / 4 },
];

export function PageNavigator({
  nodes,
  selectedPageId,
  onSelectPage,
  onStatus,
  onChanged,
  onNewFromTemplate,
}: PageNavigatorProps): JSX.Element {
  const [masters, setMasters] = useState<MasterPageInfo[]>([]);
  const [busy, setBusy] = useState<boolean>(false);
  // Picker for the "+ Add page" button (collapsed until the user opens it).
  const [adderOpen, setAdderOpen] = useState<boolean>(false);
  const [pickerSize, setPickerSize] = useState<PageSizeId>("a4");
  const [pickerOrientation, setPickerOrientation] =
    useState<PageOrientation>("portrait");
  // Right-click context menu state.
  const [menu, setMenu] = useState<{
    pageId: string;
    kind: PageCardKind;
    x: number;
    y: number;
  } | null>(null);
  // Drag-reorder state.
  const dragRef = useRef<string | null>(null);
  const [dropTargetIndex, setDropTargetIndex] = useState<number | null>(null);

  // Resolve master pages independently — they don't always live in the
  // main `nodes` tree (the bridge keeps them flagged by metadata, and
  // `list_master_pages` already sorts them by name for us). Wrapped in
  // `useCallback` so the `useEffect` below closes over the latest
  // `onStatus` prop on every render (matches the `refresh` pattern in
  // `InteractionPanel.tsx`).
  const refreshMasters = useCallback(async (): Promise<void> => {
    try {
      const list = await window.kcreate.masterPage.list();
      setMasters(list);
    } catch (e) {
      onStatus?.(`Master page list failed: ${errorMessage(e)}`);
    }
  }, [onStatus]);
  useEffect(() => {
    void refreshMasters();
    // We refresh on tree-shape changes only: the master-page list
    // only mutates when `nodes` gains or loses entries. Depending on
    // `nodes` directly would re-fire on every per-node mutation (a
    // rename, a bounds tweak, a metadata write — `refreshTree`
    // rebuilds the whole array), which would thrash the master-page
    // list for no observable change. `nodes.length` is the right
    // identity-stable proxy for "the tree shape changed", and is
    // accepted by ESLint as a primitive read.
  }, [nodes.length, refreshMasters]);

  // Build page-card data from the document tree. A "Page" is any node
  // with nodeType === "Page" that is NOT flagged as a master. We mark
  // masters that ALSO appear in the tree separately so the masters
  // section has the canonical list (from the bridge); content pages
  // come from the tree.
  const contentPages = useMemo<PageCard[]>(() => {
    const masterIdSet = new Set(masters.map((m) => m.id));
    const content: PageCard[] = [];
    for (const n of nodes) {
      if (n.nodeType !== "Page") continue;
      if (masterIdSet.has(n.id)) continue;
      content.push({
        id: n.id,
        name: n.name || "Untitled page",
        kind: "content",
        sizeLabel: describeBounds(n.bounds),
        aspect: aspectFromBounds(n.bounds),
      });
    }
    return content;
  }, [nodes, masters]);

  // Convert MasterPageInfo into a renderable card. Master pages don't
  // come with bounds (they live outside the tree if the host doesn't
  // mirror them), so we fall back to the page-size label.
  const masterCards: PageCard[] = useMemo(
    () =>
      masters.map((m) => ({
        id: m.id,
        name: m.name,
        kind: "master" as const,
        sizeLabel: describePageLayout(m.layout),
        aspect: aspectFromLayout(m.layout),
      })),
    [masters],
  );

  const handleAddPage = async (): Promise<void> => {
    setBusy(true);
    try {
      const id = await window.kcreate.layoutStudio.addPage(
        `Page ${contentPages.length + 1}`,
        pickerSize,
        pickerOrientation,
      );
      onChanged?.();
      onSelectPage(id);
      setAdderOpen(false);
    } catch (e) {
      onStatus?.(`Add page failed: ${errorMessage(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const handleDuplicate = async (pageId: string): Promise<void> => {
    setBusy(true);
    setMenu(null);
    try {
      const id = await window.kcreate.layoutStudio.duplicatePage(pageId);
      onChanged?.();
      onSelectPage(id);
    } catch (e) {
      onStatus?.(`Duplicate page failed: ${errorMessage(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const handleDelete = async (pageId: string): Promise<void> => {
    setBusy(true);
    setMenu(null);
    try {
      // The delete path lives on `document.deleteNode` for any node
      // type — pages are no exception. The bridge cascade-deletes the
      // artboards / layers underneath.
      await window.kcreate.document.deleteNode(pageId);
      onChanged?.();
    } catch (e) {
      onStatus?.(`Delete page failed: ${errorMessage(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const handleApplyMaster = async (
    contentPageId: string,
    masterPageId: string,
  ): Promise<void> => {
    setBusy(true);
    setMenu(null);
    try {
      await window.kcreate.masterPage.apply(contentPageId, masterPageId);
      onChanged?.();
    } catch (e) {
      onStatus?.(`Apply master failed: ${errorMessage(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const handleSetPageSize = async (
    pageId: string,
    size: PageSizeId,
    orientation: PageOrientation,
  ): Promise<void> => {
    setBusy(true);
    setMenu(null);
    try {
      const layout: PageLayout = {
        page_size: { kind: size },
        orientation,
        margins: {
          top_mm: 0,
          right_mm: 0,
          bottom_mm: 0,
          left_mm: 0,
        },
        master_page_id: null,
        page_number: null,
      };
      await window.kcreate.layoutStudio.setPageLayout(pageId, layout);
      onChanged?.();
    } catch (e) {
      onStatus?.(`Set page size failed: ${errorMessage(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const handleDrop = async (dropIndex: number): Promise<void> => {
    const draggedId = dragRef.current;
    dragRef.current = null;
    setDropTargetIndex(null);
    if (!draggedId) return;
    // The dropIndex / oldIndex we operate with here are
    // *content-page-relative* (indices into the filtered `contentPages`
    // array). The bridge's `reparentNode(id, null, index)`, however,
    // inserts into the document's root_ids list — which also contains
    // master pages (created via `master_page_create` → `insert_node`,
    // see `crates/kcreate_core/src/project.rs::create_master_page`).
    //
    // If we passed the content-page-relative index straight to the
    // bridge it would land in the wrong slot whenever any master page
    // is interleaved with the content pages in root order. The bot's
    // PageNavigator BUG-0001 (PR #5) caught this.
    //
    // Translation: build a list of *root-level Page node ids in
    // root_ids order* from the document tree, identify the subset
    // that is content-only, and map the content-pages-relative drop
    // index back onto the root_ids-relative slot. Using `nodes`
    // (which `document_get_tree` produces in `root_ids` order, see
    // `crates/kcreate_bridge/src/document.rs::document_get_tree`) means
    // we never have to round-trip another IPC call.
    const masterIdSet = new Set(masters.map((m) => m.id));
    const rootPageIds: string[] = [];
    for (const n of nodes) {
      if (n.parentId !== null) continue;
      if (n.nodeType !== "Page") continue;
      rootPageIds.push(n.id);
    }

    const oldContentIndex = contentPages.findIndex((p) => p.id === draggedId);
    if (oldContentIndex < 0) return;
    // Index in `contentPages` after the user drops. If they drag down
    // past their own slot, `contentPages.splice(oldContentIndex, 1)`
    // shifts everything below up by one — so the post-removal target
    // is `dropIndex - 1`.
    const newContentIndex =
      dropIndex > oldContentIndex ? dropIndex - 1 : dropIndex;
    if (newContentIndex === oldContentIndex) return;

    // Map content-page-relative -> root-list-relative.
    //
    // The picture for the bridge is the root_ids list *after* the
    // detach (the bridge always detaches first, then inserts). So we
    // model the same: take root_ids order, drop the dragged id, then
    // figure out where the bridge needs to insert so that the dragged
    // id ends up at content position `newContentIndex` once the
    // master pages are skipped.
    const rootIdsAfterDetach = rootPageIds.filter((id) => id !== draggedId);
    // Walk root_ids; count content pages we've passed; when we've
    // passed `newContentIndex` of them, that's where the insert goes.
    let rootInsertIndex = rootIdsAfterDetach.length;
    let contentSeen = 0;
    for (let i = 0; i < rootIdsAfterDetach.length; i += 1) {
      const id = rootIdsAfterDetach[i];
      if (id === undefined) continue;
      if (masterIdSet.has(id)) continue;
      if (contentSeen === newContentIndex) {
        rootInsertIndex = i;
        break;
      }
      contentSeen += 1;
    }

    setBusy(true);
    try {
      await window.kcreate.layoutStudio.reparentNode(
        draggedId,
        null,
        rootInsertIndex,
      );
      onChanged?.();
    } catch (e) {
      onStatus?.(`Reorder failed: ${errorMessage(e)}`);
    } finally {
      setBusy(false);
    }
  };

  // Dismiss the context menu on any outside click. We attach to the
  // window because the menu floats above the panel.
  useEffect(() => {
    if (!menu) return;
    const onDoc = (): void => setMenu(null);
    window.addEventListener("click", onDoc);
    return () => window.removeEventListener("click", onDoc);
  }, [menu]);

  return (
    <aside
      style={{
        width: 220,
        background: colors.bg,
        borderRight: `1px solid ${colors.border}`,
        display: "flex",
        flexDirection: "column",
        fontSize: 12,
        color: colors.text,
      }}
    >
      <div
        style={{
          flex: 1,
          overflowY: "auto",
          padding: spacing.md,
        }}
      >
        {masterCards.length > 0 ? (
          <SectionHeader label="Masters" count={masterCards.length} />
        ) : null}
        {masterCards.map((c) => (
          <Card
            key={c.id}
            card={c}
            selected={c.id === selectedPageId}
            onClick={() => onSelectPage(c.id)}
            onContextMenu={(x, y) =>
              setMenu({ pageId: c.id, kind: c.kind, x, y })
            }
          />
        ))}
        <SectionHeader label="Pages" count={contentPages.length} />
        {contentPages.map((c, i) => (
          <div key={c.id}>
            {dropTargetIndex === i ? <DropIndicator /> : null}
            <Card
              card={c}
              index={i + 1}
              selected={c.id === selectedPageId}
              onClick={() => onSelectPage(c.id)}
              onContextMenu={(x, y) =>
                setMenu({ pageId: c.id, kind: c.kind, x, y })
              }
              draggable
              onDragStart={() => {
                dragRef.current = c.id;
              }}
              onDragOver={(e) => {
                e.preventDefault();
                e.dataTransfer.dropEffect = "move";
                setDropTargetIndex(i);
              }}
              onDrop={(e) => {
                e.preventDefault();
                void handleDrop(i);
              }}
            />
          </div>
        ))}
        {/* Tail drop zone so the user can drop at the end of the list. */}
        {dropTargetIndex === contentPages.length ? <DropIndicator /> : null}
        <div
          style={{ height: 24 }}
          onDragOver={(e) => {
            e.preventDefault();
            setDropTargetIndex(contentPages.length);
          }}
          onDrop={(e) => {
            e.preventDefault();
            void handleDrop(contentPages.length);
          }}
        />
        {contentPages.length === 0 ? (
          <div style={emptyStateStyle}>
            No pages yet. Click <b>+ Add page</b> below.
          </div>
        ) : null}
      </div>
      <div
        style={{
          borderTop: `1px solid ${colors.border}`,
          padding: spacing.sm,
        }}
      >
        {adderOpen ? (
          <div
            style={{
              display: "flex",
              flexDirection: "column",
              gap: spacing.xs,
            }}
          >
            <label>
              <span style={labelTextStyle}>Size</span>
              <select
                value={pickerSize}
                onChange={(e) =>
                  setPickerSize(e.target.value as PageSizeId)
                }
                style={selectStyle}
              >
                {COMMON_SIZES.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.label}
                  </option>
                ))}
              </select>
            </label>
            <label>
              <span style={labelTextStyle}>Orientation</span>
              <select
                value={pickerOrientation}
                onChange={(e) =>
                  setPickerOrientation(e.target.value as PageOrientation)
                }
                style={selectStyle}
              >
                <option value="portrait">Portrait</option>
                <option value="landscape">Landscape</option>
              </select>
            </label>
            <div style={{ display: "flex", gap: spacing.xs }}>
              <button
                type="button"
                onClick={() => {
                  void handleAddPage();
                }}
                disabled={busy}
                style={primaryButtonStyle}
              >
                {busy ? "Adding…" : "Add"}
              </button>
              <button
                type="button"
                onClick={() => setAdderOpen(false)}
                style={secondaryButtonStyle}
              >
                Cancel
              </button>
            </div>
          </div>
        ) : (
          <div style={{ display: "flex", gap: spacing.xs }}>
            <button
              type="button"
              onClick={() => setAdderOpen(true)}
              disabled={busy}
              style={primaryButtonStyle}
            >
              + Add page
            </button>
            {onNewFromTemplate ? (
              <button
                type="button"
                onClick={onNewFromTemplate}
                disabled={busy}
                style={secondaryButtonStyle}
                title="Apply a built-in layout template"
              >
                Templates
              </button>
            ) : null}
          </div>
        )}
      </div>
      {menu ? (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          kind={menu.kind}
          pageId={menu.pageId}
          masters={masters}
          onDuplicate={(id) => {
            void handleDuplicate(id);
          }}
          onDelete={(id) => {
            void handleDelete(id);
          }}
          onApplyMaster={(contentId, masterId) => {
            void handleApplyMaster(contentId, masterId);
          }}
          onSetPageSize={(id, size, orientation) => {
            void handleSetPageSize(id, size, orientation);
          }}
          onDismiss={() => setMenu(null)}
        />
      ) : null}
    </aside>
  );
}

interface CardProps {
  card: PageCard;
  index?: number;
  selected: boolean;
  onClick: () => void;
  onContextMenu: (x: number, y: number) => void;
  draggable?: boolean;
  onDragStart?: () => void;
  onDragOver?: (e: React.DragEvent<HTMLDivElement>) => void;
  onDrop?: (e: React.DragEvent<HTMLDivElement>) => void;
}

function Card({
  card,
  index,
  selected,
  onClick,
  onContextMenu,
  draggable,
  onDragStart,
  onDragOver,
  onDrop,
}: CardProps): JSX.Element {
  // Cap aspect ratios so a 1:14-strip page doesn't push the panel
  // around. 0.4 to 2.5 covers every realistic page format.
  const clampedAspect = Math.min(Math.max(card.aspect, 0.4), 2.5);
  const thumbWidth = 160;
  const thumbHeight = Math.round(thumbWidth / clampedAspect);
  return (
    <div
      role="button"
      tabIndex={0}
      draggable={draggable ?? false}
      onClick={onClick}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onClick();
        }
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        onContextMenu(e.clientX, e.clientY);
      }}
      onDragStart={onDragStart}
      onDragOver={onDragOver}
      onDrop={onDrop}
      style={{
        display: "flex",
        flexDirection: "column",
        alignItems: "stretch",
        gap: 4,
        padding: 6,
        marginBottom: spacing.xs,
        background: selected ? colors.bgSoft : colors.bg,
        border: card.kind === "master"
          ? `1px dashed ${colors.border}`
          : `1px solid ${selected ? colors.accent : colors.border}`,
        borderRadius: radius.card,
        cursor: "pointer",
      }}
    >
      <div
        style={{
          width: thumbWidth,
          height: thumbHeight,
          background: "#FAFAFA",
          border: `1px solid ${colors.border}`,
          borderRadius: 4,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          color: colors.textMuted,
          fontSize: 10,
        }}
      >
        {card.sizeLabel}
      </div>
      <div
        style={{
          display: "flex",
          alignItems: "baseline",
          justifyContent: "space-between",
          gap: 4,
        }}
      >
        <span
          style={{
            fontSize: 11,
            fontWeight: 500,
            color: selected ? colors.accent : colors.text,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
        >
          {card.name}
        </span>
        {index !== undefined ? (
          <span
            style={{
              fontSize: 9,
              color: colors.textMuted,
              flexShrink: 0,
            }}
          >
            #{index}
          </span>
        ) : null}
      </div>
    </div>
  );
}

function DropIndicator(): JSX.Element {
  return (
    <div
      style={{
        height: 2,
        background: colors.accent,
        margin: `${spacing.xs}px 6px`,
        borderRadius: 1,
      }}
    />
  );
}

function SectionHeader({
  label,
  count,
}: {
  label: string;
  count: number;
}): JSX.Element {
  return (
    <div
      style={{
        fontSize: 10,
        fontWeight: 600,
        color: colors.textMuted,
        textTransform: "uppercase",
        letterSpacing: 0.5,
        margin: `${spacing.xs}px 0`,
      }}
    >
      {label} · {count}
    </div>
  );
}

interface ContextMenuProps {
  x: number;
  y: number;
  kind: PageCardKind;
  pageId: string;
  masters: MasterPageInfo[];
  onDuplicate: (id: string) => void;
  onDelete: (id: string) => void;
  onApplyMaster: (contentId: string, masterId: string) => void;
  onSetPageSize: (
    id: string,
    size: PageSizeId,
    orientation: PageOrientation,
  ) => void;
  onDismiss: () => void;
}

function ContextMenu({
  x,
  y,
  kind,
  pageId,
  masters,
  onDuplicate,
  onDelete,
  onApplyMaster,
  onSetPageSize,
  onDismiss,
}: ContextMenuProps): JSX.Element {
  return (
    <div
      role="menu"
      onClick={(e) => e.stopPropagation()}
      style={{
        position: "fixed",
        left: x,
        top: y,
        background: colors.bg,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.card,
        boxShadow: "0 4px 12px rgba(0,0,0,0.08)",
        padding: spacing.xs,
        minWidth: 200,
        zIndex: 1000,
        fontSize: 12,
      }}
    >
      <MenuItem
        label="Duplicate"
        onClick={() => {
          onDuplicate(pageId);
          onDismiss();
        }}
      />
      <MenuItem
        label="Delete"
        danger
        onClick={() => {
          onDelete(pageId);
          onDismiss();
        }}
      />
      {kind === "content" ? (
        <>
          <MenuDivider />
          <MenuSubheading label="Set page size" />
          {COMMON_SIZES.map((s) => (
            <MenuItem
              key={s.id}
              label={`${s.label} portrait`}
              onClick={() => {
                onSetPageSize(pageId, s.id, "portrait");
                onDismiss();
              }}
            />
          ))}
          {masters.length > 0 ? (
            <>
              <MenuDivider />
              <MenuSubheading label="Apply master" />
              {masters.map((m) => (
                <MenuItem
                  key={m.id}
                  label={m.name}
                  onClick={() => {
                    onApplyMaster(pageId, m.id);
                    onDismiss();
                  }}
                />
              ))}
            </>
          ) : null}
        </>
      ) : null}
    </div>
  );
}

function MenuItem({
  label,
  onClick,
  danger,
}: {
  label: string;
  onClick: () => void;
  danger?: boolean;
}): JSX.Element {
  return (
    <button
      type="button"
      role="menuitem"
      onClick={onClick}
      style={{
        display: "block",
        width: "100%",
        textAlign: "left",
        padding: "6px 10px",
        background: "transparent",
        border: "none",
        cursor: "pointer",
        color: danger ? "#B91C1C" : colors.text,
        borderRadius: 6,
        fontSize: 12,
      }}
    >
      {label}
    </button>
  );
}

function MenuDivider(): JSX.Element {
  return (
    <div
      style={{
        height: 1,
        background: colors.border,
        margin: `${spacing.xs}px 0`,
      }}
    />
  );
}

function MenuSubheading({ label }: { label: string }): JSX.Element {
  return (
    <div
      style={{
        padding: "2px 10px",
        fontSize: 10,
        fontWeight: 600,
        color: colors.textMuted,
        textTransform: "uppercase",
        letterSpacing: 0.5,
      }}
    >
      {label}
    </div>
  );
}

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

function describeBounds(b: NodeInfo["bounds"]): string {
  if (b.width <= 0 || b.height <= 0) return "Unsized";
  return `${Math.round(b.width)} × ${Math.round(b.height)}`;
}

function aspectFromBounds(b: NodeInfo["bounds"]): number {
  if (b.height <= 0) return 1;
  return b.width / b.height;
}

function describePageLayout(layout: PageLayout | null): string {
  if (!layout) return "Master";
  const sizeLabel = describePageSize(layout.page_size);
  return `${sizeLabel} ${layout.orientation}`;
}

function describePageSize(s: PageSize): string {
  if (s.kind === "custom") {
    return `${Math.round(s.width_mm)} × ${Math.round(s.height_mm)} mm`;
  }
  const meta = COMMON_SIZES.find((m) => m.id === s.kind);
  return meta?.label ?? s.kind;
}

function aspectFromLayout(layout: PageLayout | null): number {
  if (!layout) return 210 / 297;
  if (layout.page_size.kind === "custom") {
    const { width_mm, height_mm } = layout.page_size;
    if (height_mm <= 0) return 1;
    return layout.orientation === "portrait"
      ? width_mm / height_mm
      : height_mm / width_mm;
  }
  const meta = COMMON_SIZES.find((m) => m.id === layout.page_size.kind);
  const a = meta?.aspectPortrait ?? 210 / 297;
  return layout.orientation === "portrait" ? a : 1 / a;
}

function errorMessage(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

// -----------------------------------------------------------------------------
// Styles
// -----------------------------------------------------------------------------

const emptyStateStyle: React.CSSProperties = {
  padding: spacing.md,
  color: colors.textMuted,
  fontSize: 11,
  textAlign: "center",
};

const labelTextStyle: React.CSSProperties = {
  display: "block",
  fontSize: 10,
  fontWeight: 600,
  color: colors.textMuted,
  marginBottom: 2,
  textTransform: "uppercase",
  letterSpacing: 0.5,
};

const selectStyle: React.CSSProperties = {
  width: "100%",
  padding: "4px 6px",
  fontSize: 11,
  border: `1px solid ${colors.border}`,
  borderRadius: 6,
  background: colors.bg,
  color: colors.text,
};

const primaryButtonStyle: React.CSSProperties = {
  flex: 1,
  padding: "6px 10px",
  fontSize: 11,
  fontWeight: 500,
  background: colors.accent,
  color: colors.textInverse,
  border: "none",
  borderRadius: radius.pill,
  cursor: "pointer",
};

const secondaryButtonStyle: React.CSSProperties = {
  flex: 1,
  padding: "6px 10px",
  fontSize: 11,
  fontWeight: 500,
  background: "transparent",
  color: colors.textMuted,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.pill,
  cursor: "pointer",
};

import { useEffect, useRef, useState } from "react";

import type { ArtboardInfo } from "../../../shared/scene";
import { colors, font, radius, spacing } from "../styles/tokens";

export interface ArtboardPanelProps {
  /** Artboards to list. Parent owns the polling/refresh cadence. */
  artboards: ArtboardInfo[];
  /** Currently selected node id (used to highlight the matching card). */
  selectedId: string | null;

  /**
   * "New artboard" button opens the parent's dialog. Receiving a click
   * here lets the panel stay presentational; the dialog logic lives
   * up the tree (it's reused from the home page wiring too).
   */
  onRequestCreate: () => void;

  /**
   * Focus the canvas viewport on the given artboard. Implementation
   * lives in the parent because it needs the canvas dimensions to
   * compute the right pan + zoom.
   */
  onFocusArtboard: (artboard: ArtboardInfo) => void;

  /** Rename an artboard. Empty/whitespace names are rejected by the
   * panel before this is called. */
  onRenameArtboard: (id: string, name: string) => void;

  /** Duplicate the artboard. Bridge offsets the copy by width + 100px. */
  onDuplicateArtboard: (id: string) => void;

  /** Resize the artboard. The panel surfaces a small inline form when
   * the user picks "Resize" from the context menu. */
  onResizeArtboard: (id: string, width: number, height: number) => void;

  /**
   * Magic Resize the artboard — opens the parent's multi-size dialog.
   * The reflow logic + IPC live up the tree (the dialog needs the
   * preset catalogue the parent already holds).
   */
  onMagicResize: (id: string) => void;

  /** Delete the artboard + its subtree. */
  onDeleteArtboard: (id: string) => void;
}

/// Inline resize form state. Stored locally so we don't leak transient
/// UI state up to the parent — the bridge call only fires when the
/// user actually commits the resize.
interface ResizeDraft {
  artboardId: string;
  width: number;
  height: number;
}

export function ArtboardPanel(props: ArtboardPanelProps): JSX.Element {
  const {
    artboards,
    selectedId,
    onRequestCreate,
    onFocusArtboard,
    onRenameArtboard,
    onDuplicateArtboard,
    onResizeArtboard,
    onMagicResize,
    onDeleteArtboard,
  } = props;
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [draftName, setDraftName] = useState("");
  const [menu, setMenu] = useState<{ x: number; y: number; id: string } | null>(
    null,
  );
  const [resizeDraft, setResizeDraft] = useState<ResizeDraft | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  // Dismiss the context menu on any outside click + Escape.
  useEffect(() => {
    if (!menu) return;
    const onDown = (e: MouseEvent): void => {
      if (
        menuRef.current &&
        e.target instanceof Node &&
        !menuRef.current.contains(e.target)
      ) {
        setMenu(null);
      }
    };
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") setMenu(null);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [menu]);

  const commitRename = (id: string): void => {
    const t = draftName.trim();
    if (t.length > 0) onRenameArtboard(id, t);
    setRenamingId(null);
    setDraftName("");
  };

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.sm,
        fontFamily: font.family,
      }}
    >
      <button type="button" onClick={onRequestCreate} style={newButton}>
        + New artboard
      </button>
      {artboards.length === 0 ? (
        <div
          style={{
            padding: spacing.md,
            fontSize: 12,
            color: colors.textMuted,
            lineHeight: 1.5,
          }}
        >
          No artboards yet. Click <strong>+ New artboard</strong> to add one.
        </div>
      ) : (
        <ul
          style={{
            listStyle: "none",
            margin: 0,
            padding: 0,
            display: "flex",
            flexDirection: "column",
            gap: spacing.xs,
          }}
        >
          {artboards.map((a) => {
            const selected = a.id === selectedId;
            const isRenaming = renamingId === a.id;
            return (
              <li key={a.id}>
                <div
                  onClick={() => {
                    if (!isRenaming) onFocusArtboard(a);
                  }}
                  onDoubleClick={(e) => {
                    e.stopPropagation();
                    setRenamingId(a.id);
                    setDraftName(a.name);
                  }}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    setMenu({ x: e.clientX, y: e.clientY, id: a.id });
                  }}
                  style={{
                    border: `1px solid ${selected ? colors.accent : colors.border}`,
                    borderRadius: radius.card / 2,
                    padding: spacing.sm,
                    background: selected ? colors.bgSoft : colors.bg,
                    cursor: "pointer",
                    display: "flex",
                    flexDirection: "column",
                    gap: spacing.xs,
                  }}
                >
                  <Thumbnail width={a.width} height={a.height} />
                  {isRenaming ? (
                    <input
                      autoFocus
                      value={draftName}
                      onChange={(e) => setDraftName(e.target.value)}
                      onBlur={() => commitRename(a.id)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") commitRename(a.id);
                        if (e.key === "Escape") {
                          setRenamingId(null);
                          setDraftName("");
                        }
                      }}
                      onClick={(e) => e.stopPropagation()}
                      style={renameInputStyle}
                    />
                  ) : (
                    <span
                      style={{
                        fontSize: 13,
                        fontWeight: 500,
                        color: selected ? colors.accent : colors.text,
                      }}
                    >
                      {a.name}
                    </span>
                  )}
                  <span style={{ fontSize: 11, color: colors.textMuted }}>
                    {Math.round(a.width)} × {Math.round(a.height)}
                  </span>
                </div>
              </li>
            );
          })}
        </ul>
      )}
      {menu ? (
        <ContextMenuView
          menuRef={menuRef}
          x={menu.x}
          y={menu.y}
          items={[
            {
              label: "Rename",
              onClick: () => {
                const a = artboards.find((x) => x.id === menu.id);
                if (a) {
                  setRenamingId(a.id);
                  setDraftName(a.name);
                }
                setMenu(null);
              },
            },
            {
              label: "Duplicate",
              onClick: () => {
                onDuplicateArtboard(menu.id);
                setMenu(null);
              },
            },
            {
              label: "Resize…",
              onClick: () => {
                const a = artboards.find((x) => x.id === menu.id);
                if (a) {
                  setResizeDraft({
                    artboardId: a.id,
                    width: a.width,
                    height: a.height,
                  });
                }
                setMenu(null);
              },
            },
            {
              label: "Magic Resize…",
              onClick: () => {
                onMagicResize(menu.id);
                setMenu(null);
              },
            },
            {
              label: "Delete",
              destructive: true,
              onClick: () => {
                onDeleteArtboard(menu.id);
                setMenu(null);
              },
            },
          ]}
        />
      ) : null}
      {resizeDraft ? (
        <ResizeDialog
          draft={resizeDraft}
          onCancel={() => setResizeDraft(null)}
          onCommit={(w, h) => {
            onResizeArtboard(resizeDraft.artboardId, w, h);
            setResizeDraft(null);
          }}
        />
      ) : null}
    </div>
  );
}

function Thumbnail({
  width,
  height,
}: {
  width: number;
  height: number;
}): JSX.Element {
  // 80×60 box, preserve aspect ratio of the artboard inside it.
  const boxW = 80;
  const boxH = 60;
  const scale = Math.min(boxW / width, boxH / height);
  const w = Math.max(8, width * scale);
  const h = Math.max(8, height * scale);
  return (
    <div
      style={{
        width: boxW,
        height: boxH,
        background: colors.bgSoft,
        borderRadius: radius.card / 3,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
      }}
    >
      <div
        style={{
          width: w,
          height: h,
          background: colors.bg,
          border: `1px solid ${colors.border}`,
          borderRadius: 2,
        }}
      />
    </div>
  );
}

interface ContextMenuItem {
  label: string;
  onClick: () => void;
  destructive?: boolean;
}

function ContextMenuView({
  menuRef,
  x,
  y,
  items,
}: {
  menuRef: React.RefObject<HTMLDivElement>;
  x: number;
  y: number;
  items: ContextMenuItem[];
}): JSX.Element {
  return (
    <div
      ref={menuRef}
      role="menu"
      style={{
        position: "fixed",
        top: y,
        left: x,
        background: colors.bg,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.card / 2,
        boxShadow: "0 6px 18px rgba(0,0,0,0.18)",
        padding: 4,
        zIndex: 999,
        minWidth: 140,
      }}
    >
      {items.map((it) => (
        <button
          key={it.label}
          type="button"
          role="menuitem"
          onClick={it.onClick}
          style={{
            display: "block",
            width: "100%",
            textAlign: "left",
            background: "transparent",
            border: "none",
            color: it.destructive ? "#B91C1C" : colors.text,
            padding: "6px 10px",
            fontSize: 12,
            cursor: "pointer",
            borderRadius: 3,
          }}
        >
          {it.label}
        </button>
      ))}
    </div>
  );
}

function ResizeDialog({
  draft,
  onCancel,
  onCommit,
}: {
  draft: ResizeDraft;
  onCancel: () => void;
  onCommit: (w: number, h: number) => void;
}): JSX.Element {
  const [w, setW] = useState(draft.width);
  const [h, setH] = useState(draft.height);
  const submit = (): void => {
    if (!Number.isFinite(w) || w <= 0 || !Number.isFinite(h) || h <= 0) return;
    onCommit(w, h);
  };
  return (
    <div
      role="dialog"
      aria-modal="true"
      onClick={onCancel}
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(15,18,25,0.45)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        zIndex: 1100,
        fontFamily: font.family,
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          width: 280,
          background: colors.bg,
          color: colors.text,
          borderRadius: radius.card,
          padding: spacing.lg,
          display: "flex",
          flexDirection: "column",
          gap: spacing.md,
        }}
      >
        <h3 style={{ margin: 0, fontSize: 14, fontWeight: 600 }}>
          Resize artboard
        </h3>
        <label style={fieldLabel}>
          Width (px)
          <input
            type="number"
            min={1}
            value={w}
            onChange={(e) => setW(Number(e.target.value))}
            style={renameInputStyle}
          />
        </label>
        <label style={fieldLabel}>
          Height (px)
          <input
            type="number"
            min={1}
            value={h}
            onChange={(e) => setH(Number(e.target.value))}
            style={renameInputStyle}
          />
        </label>
        <div
          style={{
            display: "flex",
            gap: spacing.sm,
            justifyContent: "flex-end",
          }}
        >
          <button type="button" onClick={onCancel} style={secondaryButton}>
            Cancel
          </button>
          <button type="button" onClick={submit} style={primaryButton}>
            Resize
          </button>
        </div>
      </div>
    </div>
  );
}

const newButton: React.CSSProperties = {
  display: "block",
  width: "100%",
  background: colors.accent,
  color: colors.textInverse,
  border: "none",
  borderRadius: radius.card / 2,
  padding: `${spacing.xs}px ${spacing.sm}px`,
  fontSize: 12,
  fontWeight: 500,
  cursor: "pointer",
};

const renameInputStyle: React.CSSProperties = {
  fontFamily: font.family,
  background: colors.bg,
  color: colors.text,
  border: `1px solid ${colors.accent}`,
  borderRadius: 3,
  padding: "2px 6px",
  fontSize: 12,
};

const fieldLabel: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.xs,
  fontSize: 12,
  color: colors.textMuted,
};

const primaryButton: React.CSSProperties = {
  background: colors.accent,
  color: colors.textInverse,
  border: "none",
  borderRadius: radius.card / 2,
  padding: `${spacing.xs}px ${spacing.md}px`,
  fontSize: 13,
  fontWeight: 500,
  cursor: "pointer",
};

const secondaryButton: React.CSSProperties = {
  background: "transparent",
  color: colors.text,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card / 2,
  padding: `${spacing.xs}px ${spacing.md}px`,
  fontSize: 13,
  cursor: "pointer",
};

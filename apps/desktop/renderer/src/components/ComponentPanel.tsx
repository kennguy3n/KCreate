import { useEffect, useRef, useState } from "react";

import type { ComponentInfo, NodeInfo } from "../../../shared/scene";
import { colors, font, radius, spacing } from "../styles/tokens";

export interface ComponentPanelProps {
  /** Registered component definitions. Parent owns refresh cadence. */
  components: ComponentInfo[];
  /**
   * Selected node ids. Used to gate "Create component from selection"
   * — the operation requires a sibling-flat selection of one or more
   * nodes, so we surface an explanation when nothing is selected.
   */
  selectedNodeIds: string[];

  /**
   * Currently selected node, if any. When this is a `ComponentLayer`
   * we show the variant switcher and the Detach action.
   */
  selectedNode: NodeInfo | null;

  /**
   * Convert the current selection into a new component. The parent
   * resolves the selection itself; this callback just kicks off the
   * IPC + refresh dance.
   */
  onCreateFromSelection: (name: string) => void;

  /**
   * Instantiate the given component at the document origin (or under
   * the current artboard, depending on the host wiring).
   */
  onInstantiate: (componentId: string) => void;

  /** Add a fresh variant to an existing definition. */
  onAddVariant: (componentId: string, name: string) => void;

  /** Switch which variant a placed instance displays. */
  onSwitchVariant: (nodeId: string, variantId: string) => void;

  /** Detach the currently-selected component instance. */
  onDetach: (nodeId: string) => void;
}

/**
 * The "Components" view in the left panel's Assets tab. Lists every
 * registered `ComponentDefinition` with a thumbnail card; clicking
 * "Place" instantiates it, right-clicking opens the lifecycle menu.
 *
 * Visual language follows `ArtboardPanel` — same card metrics, same
 * context menu styling, same KChat tokens. This component is
 * intentionally presentational; all the bridge calls and state
 * synchronization live in the parent (`EditorPage`).
 */
export function ComponentPanel(props: ComponentPanelProps): JSX.Element {
  const {
    components,
    selectedNodeIds,
    selectedNode,
    onCreateFromSelection,
    onInstantiate,
    onAddVariant,
    onSwitchVariant,
    onDetach,
  } = props;

  const [menu, setMenu] = useState<{
    x: number;
    y: number;
    id: string;
  } | null>(null);
  const [creating, setCreating] = useState(false);
  const [draftName, setDraftName] = useState("");
  const [variantDraft, setVariantDraft] = useState<{
    componentId: string;
    name: string;
  } | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  // Dismiss the context menu on outside click + Escape, matching the
  // ArtboardPanel UX.
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

  const selectionEmpty = selectedNodeIds.length === 0;
  const selectedIsInstance = selectedNode?.nodeType === "ComponentLayer";

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.sm,
        fontFamily: font.family,
      }}
    >
      {creating ? (
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            gap: spacing.xs,
            padding: spacing.sm,
            border: `1px solid ${colors.border}`,
            borderRadius: radius.card / 2,
            background: colors.bgSoft,
          }}
        >
          <label style={labelStyle} htmlFor="component-name-input">
            Component name
          </label>
          <input
            id="component-name-input"
            autoFocus
            value={draftName}
            onChange={(e) => setDraftName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                const t = draftName.trim();
                if (t.length > 0) {
                  onCreateFromSelection(t);
                  setCreating(false);
                  setDraftName("");
                }
              } else if (e.key === "Escape") {
                setCreating(false);
                setDraftName("");
              }
            }}
            style={inputStyle}
            placeholder="Button"
          />
          <div style={{ display: "flex", gap: spacing.xs }}>
            <button
              type="button"
              onClick={() => {
                const t = draftName.trim();
                if (t.length > 0) {
                  onCreateFromSelection(t);
                  setCreating(false);
                  setDraftName("");
                }
              }}
              style={primaryButton}
              disabled={draftName.trim().length === 0}
            >
              Create
            </button>
            <button
              type="button"
              onClick={() => {
                setCreating(false);
                setDraftName("");
              }}
              style={secondaryButton}
            >
              Cancel
            </button>
          </div>
        </div>
      ) : (
        <button
          type="button"
          onClick={() => setCreating(true)}
          disabled={selectionEmpty}
          title={
            selectionEmpty
              ? "Select one or more sibling nodes to create a component"
              : "Create component from selection"
          }
          style={selectionEmpty ? newButtonDisabled : newButton}
        >
          + Create from selection
        </button>
      )}

      {selectedIsInstance && selectedNode ? (
        <InstanceControls
          instance={selectedNode}
          components={components}
          onSwitchVariant={onSwitchVariant}
          onDetach={onDetach}
        />
      ) : null}

      {components.length === 0 ? (
        <div
          style={{
            padding: spacing.md,
            fontSize: 12,
            color: colors.textMuted,
            lineHeight: 1.5,
          }}
        >
          No components yet. Select something on the canvas, then click{" "}
          <strong>+ Create from selection</strong>.
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
          {components.map((c) => (
            <li key={c.id}>
              <div
                onClick={() => onInstantiate(c.id)}
                onContextMenu={(e) => {
                  e.preventDefault();
                  setMenu({ x: e.clientX, y: e.clientY, id: c.id });
                }}
                style={{
                  border: `1px solid ${colors.border}`,
                  borderRadius: radius.card / 2,
                  padding: spacing.sm,
                  background: colors.bg,
                  cursor: "pointer",
                  display: "flex",
                  flexDirection: "column",
                  gap: spacing.xs,
                }}
                title="Click to place an instance"
              >
                <ComponentThumbnail />
                <span
                  style={{
                    fontSize: 13,
                    fontWeight: 500,
                    color: colors.text,
                  }}
                >
                  {c.name}
                </span>
                <span style={{ fontSize: 11, color: colors.textMuted }}>
                  {c.variants.length} variant
                  {c.variants.length === 1 ? "" : "s"}
                </span>
              </div>
              {variantDraft?.componentId === c.id ? (
                <div
                  style={{
                    display: "flex",
                    gap: spacing.xs,
                    marginTop: spacing.xs,
                  }}
                >
                  <input
                    autoFocus
                    value={variantDraft.name}
                    onChange={(e) =>
                      setVariantDraft({
                        componentId: c.id,
                        name: e.target.value,
                      })
                    }
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        const t = variantDraft.name.trim();
                        if (t.length > 0) {
                          onAddVariant(c.id, t);
                          setVariantDraft(null);
                        }
                      } else if (e.key === "Escape") {
                        setVariantDraft(null);
                      }
                    }}
                    placeholder="Hover"
                    style={inputStyle}
                  />
                  <button
                    type="button"
                    onClick={() => {
                      const t = variantDraft.name.trim();
                      if (t.length > 0) {
                        onAddVariant(c.id, t);
                        setVariantDraft(null);
                      }
                    }}
                    style={primaryButton}
                  >
                    Add
                  </button>
                </div>
              ) : null}
            </li>
          ))}
        </ul>
      )}

      {menu ? (
        <ContextMenuView
          menuRef={menuRef}
          x={menu.x}
          y={menu.y}
          items={[
            {
              label: "Place instance",
              onClick: () => {
                onInstantiate(menu.id);
                setMenu(null);
              },
            },
            {
              label: "Add variant…",
              onClick: () => {
                setVariantDraft({ componentId: menu.id, name: "" });
                setMenu(null);
              },
            },
          ]}
        />
      ) : null}
    </div>
  );
}

function InstanceControls({
  instance,
  components,
  onSwitchVariant,
  onDetach,
}: {
  instance: NodeInfo;
  components: ComponentInfo[];
  onSwitchVariant: (nodeId: string, variantId: string) => void;
  onDetach: (nodeId: string) => void;
}): JSX.Element | null {
  // Component-instance state is surfaced on the wire NodeInfo so we
  // don't have to re-parse a raw metadata string here. The bridge
  // populates `componentInstance` iff `nodeType === "ComponentLayer"`
  // and the metadata carries a parseable payload (see
  // `kcreate_bridge::document::NodeInfo::From<&Node>`).
  const inst = instance.componentInstance;
  if (!inst) return null;
  const def = components.find((c) => c.id === inst.definitionId);
  if (!def) return null;
  const activeVariantId = inst.activeVariantId;
  return (
    <div
      style={{
        border: `1px solid ${colors.accent}`,
        borderRadius: radius.card / 2,
        padding: spacing.sm,
        background: colors.bgSoft,
        display: "flex",
        flexDirection: "column",
        gap: spacing.xs,
      }}
    >
      <div
        style={{ fontSize: 12, fontWeight: 600, color: colors.text }}
      >
        {def.name}
      </div>
      <label style={labelStyle} htmlFor="variant-select">
        Variant
      </label>
      <select
        id="variant-select"
        value={activeVariantId}
        onChange={(e) => onSwitchVariant(instance.id, e.target.value)}
        style={inputStyle}
      >
        {def.variants.map((v) => (
          <option key={v.id} value={v.id}>
            {v.name}
          </option>
        ))}
      </select>
      <button
        type="button"
        onClick={() => onDetach(instance.id)}
        style={secondaryButton}
      >
        Detach instance
      </button>
    </div>
  );
}

function ComponentThumbnail(): JSX.Element {
  return (
    <div
      style={{
        width: 80,
        height: 60,
        background: colors.bgSoft,
        borderRadius: radius.card / 3,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        color: colors.accent,
        fontSize: 18,
        fontWeight: 700,
      }}
    >
      ◇
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
        minWidth: 160,
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
            padding: "6px 10px",
            fontSize: 12,
            color: it.destructive ? "#B91C1C" : colors.text,
            background: "transparent",
            border: "none",
            cursor: "pointer",
            borderRadius: 4,
          }}
          onMouseEnter={(e) => {
            (e.currentTarget as HTMLButtonElement).style.background =
              colors.bgSoft;
          }}
          onMouseLeave={(e) => {
            (e.currentTarget as HTMLButtonElement).style.background =
              "transparent";
          }}
        >
          {it.label}
        </button>
      ))}
    </div>
  );
}

const newButton: React.CSSProperties = {
  padding: "8px 12px",
  fontSize: 13,
  fontWeight: 500,
  background: colors.accent,
  color: colors.textInverse,
  border: "none",
  borderRadius: radius.card / 2,
  cursor: "pointer",
};

const newButtonDisabled: React.CSSProperties = {
  ...newButton,
  background: colors.border,
  color: colors.textMuted,
  cursor: "not-allowed",
};

const primaryButton: React.CSSProperties = {
  padding: "6px 10px",
  fontSize: 12,
  fontWeight: 500,
  background: colors.accent,
  color: colors.textInverse,
  border: "none",
  borderRadius: radius.card / 2,
  cursor: "pointer",
};

const secondaryButton: React.CSSProperties = {
  padding: "6px 10px",
  fontSize: 12,
  background: colors.bg,
  color: colors.text,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card / 2,
  cursor: "pointer",
};

const inputStyle: React.CSSProperties = {
  flex: 1,
  padding: "6px 8px",
  fontSize: 12,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card / 3,
  background: colors.bg,
  color: colors.text,
  outline: "none",
};

const labelStyle: React.CSSProperties = {
  fontSize: 11,
  fontWeight: 500,
  color: colors.textMuted,
  textTransform: "uppercase",
  letterSpacing: 0.5,
};

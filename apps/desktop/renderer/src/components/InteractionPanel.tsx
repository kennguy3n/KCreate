// InteractionPanel — Phase 1, Block A, Task 3.
//
// Prototype-mode right-panel face. Lists interactions attached to the
// currently selected node and lets the user add new click/hover/press
// triggers, each pointing at a target artboard (or `Back` /
// `CloseOverlay` for navigation). Backed by the
// `window.kcreate.interaction` bridge (Block A Task 2).
//
// Real implementation — there is no mock data, no stub. Every render
// reflects the live bridge state for the selected node.

import { useCallback, useEffect, useState } from "react";

import type {
  Interaction,
  InteractionAction,
  InteractionTrigger,
  NodeInfo,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface InteractionPanelProps {
  selected: NodeInfo | null;
  artboards: Array<{ id: string; name: string }>;
  onStatus?: (msg: string | null) => void;
  /** Called after every successful add / remove. The host typically
   * refreshes the document tree so lightning-bolt indicators in the
   * layer panel stay accurate. */
  onChanged?: () => void;
}

const TRIGGERS: ReadonlyArray<{ id: InteractionTrigger; label: string }> = [
  { id: "click", label: "Click" },
  { id: "hover", label: "Hover" },
  { id: "press", label: "Press" },
];

type ActionKind = InteractionAction["kind"];

const ACTION_KINDS: ReadonlyArray<{ id: ActionKind; label: string }> = [
  { id: "navigate_to", label: "Navigate to artboard" },
  { id: "scroll_to", label: "Scroll to node" },
  { id: "open_overlay", label: "Open overlay" },
  { id: "close_overlay", label: "Close overlay" },
  { id: "back", label: "Back" },
];

export function InteractionPanel({
  selected,
  artboards,
  onStatus,
  onChanged,
}: InteractionPanelProps): JSX.Element {
  const [items, setItems] = useState<Interaction[]>([]);
  const [trigger, setTrigger] = useState<InteractionTrigger>("click");
  const [actionKind, setActionKind] = useState<ActionKind>("navigate_to");
  const [targetId, setTargetId] = useState<string>("");
  const [busy, setBusy] = useState<boolean>(false);

  const refresh = useCallback(async (): Promise<void> => {
    if (!selected) {
      setItems([]);
      return;
    }
    try {
      const list = await window.kcreate.interaction.list(selected.id);
      setItems(list);
    } catch (e) {
      onStatus?.(`Interaction list failed: ${errorMessage(e)}`);
    }
  }, [selected, onStatus]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Default the target artboard to the first one whenever the action
  // kind switches to one that needs an artboard.
  useEffect(() => {
    if (
      (actionKind === "navigate_to" || actionKind === "open_overlay") &&
      targetId === ""
    ) {
      const first = artboards[0];
      if (first) setTargetId(first.id);
    }
  }, [actionKind, targetId, artboards]);

  if (!selected) {
    return (
      <div style={emptyStateStyle}>
        Select a layer to manage its interactions.
      </div>
    );
  }

  const needsTarget = actionKind === "navigate_to" || actionKind === "open_overlay";
  const canAdd =
    !busy && (!needsTarget || (needsTarget && targetId !== ""));

  const handleAdd = async (): Promise<void> => {
    if (!canAdd) return;
    setBusy(true);
    try {
      const action = buildAction(actionKind, targetId);
      await window.kcreate.interaction.add(selected.id, trigger, action);
      onStatus?.("Interaction added.");
      await refresh();
      onChanged?.();
    } catch (e) {
      onStatus?.(`Interaction add failed: ${errorMessage(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const handleRemove = async (interactionId: string): Promise<void> => {
    setBusy(true);
    try {
      await window.kcreate.interaction.remove(selected.id, interactionId);
      onStatus?.("Interaction removed.");
      await refresh();
      onChanged?.();
    } catch (e) {
      onStatus?.(`Interaction remove failed: ${errorMessage(e)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div style={containerStyle}>
      <div style={headerStyle}>
        <strong>{selected.name}</strong>
        <span style={countBadge}>{items.length}</span>
      </div>
      <p style={paragraphStyle}>
        Interactions defined on this layer fire in Prototype play mode.
      </p>

      {items.length === 0 ? (
        <div style={emptyStateStyle}>
          No interactions yet — add one below.
        </div>
      ) : (
        <ul style={listStyle} aria-label="Interactions on this layer">
          {items.map((it) => (
            <li key={it.id} style={itemStyle}>
              <div style={itemHeaderStyle}>
                <span style={pillStyle}>{it.trigger}</span>
                <span style={actionTextStyle}>
                  {describeAction(it.action, artboards)}
                </span>
              </div>
              <button
                type="button"
                onClick={() => {
                  void handleRemove(it.id);
                }}
                disabled={busy}
                style={removeBtn(busy)}
                aria-label="Remove interaction"
              >
                Remove
              </button>
            </li>
          ))}
        </ul>
      )}

      <section style={addCardStyle}>
        <div style={cardHeaderStyle}>
          <strong>Add interaction</strong>
        </div>
        <label style={fieldStyle}>
          <span style={labelTextStyle}>Trigger</span>
          <select
            value={trigger}
            onChange={(e) =>
              setTrigger(e.target.value as InteractionTrigger)
            }
            style={selectStyle}
          >
            {TRIGGERS.map((t) => (
              <option key={t.id} value={t.id}>
                {t.label}
              </option>
            ))}
          </select>
        </label>
        <label style={fieldStyle}>
          <span style={labelTextStyle}>Action</span>
          <select
            value={actionKind}
            onChange={(e) => setActionKind(e.target.value as ActionKind)}
            style={selectStyle}
          >
            {ACTION_KINDS.map((a) => (
              <option key={a.id} value={a.id}>
                {a.label}
              </option>
            ))}
          </select>
        </label>
        {needsTarget ? (
          <label style={fieldStyle}>
            <span style={labelTextStyle}>Target artboard</span>
            <select
              value={targetId}
              onChange={(e) => setTargetId(e.target.value)}
              style={selectStyle}
            >
              {artboards.length === 0 ? (
                <option value="">— no artboards —</option>
              ) : (
                artboards.map((a) => (
                  <option key={a.id} value={a.id}>
                    {a.name}
                  </option>
                ))
              )}
            </select>
          </label>
        ) : null}
        <button
          type="button"
          onClick={() => {
            void handleAdd();
          }}
          disabled={!canAdd}
          style={primaryBtn(!canAdd)}
        >
          {busy ? "Saving…" : "Add interaction"}
        </button>
      </section>
    </div>
  );
}

function buildAction(kind: ActionKind, targetId: string): InteractionAction {
  switch (kind) {
    case "navigate_to":
      return { kind: "navigate_to", target_artboard_id: targetId };
    case "scroll_to":
      return { kind: "scroll_to", target_node_id: targetId };
    case "open_overlay":
      return { kind: "open_overlay", overlay_artboard_id: targetId };
    case "close_overlay":
      return { kind: "close_overlay" };
    case "back":
      return { kind: "back" };
  }
}

function describeAction(
  action: InteractionAction,
  artboards: Array<{ id: string; name: string }>,
): string {
  const lookup = (id: string): string => {
    const ab = artboards.find((a) => a.id === id);
    return ab ? ab.name : `${id.slice(0, 8)}…`;
  };
  switch (action.kind) {
    case "navigate_to":
      return `Navigate to ${lookup(action.target_artboard_id)}`;
    case "scroll_to":
      return `Scroll to ${action.target_node_id.slice(0, 8)}…`;
    case "open_overlay":
      return `Open overlay ${lookup(action.overlay_artboard_id)}`;
    case "close_overlay":
      return "Close overlay";
    case "back":
      return "Back";
  }
}

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

const containerStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.sm,
};

const headerStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  fontSize: 13,
  color: colors.text,
};

const countBadge: React.CSSProperties = {
  background: colors.bgSoft,
  color: colors.accent,
  fontSize: 11,
  fontWeight: 600,
  padding: "2px 8px",
  borderRadius: radius.pill,
};

const paragraphStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 11,
  color: colors.textMuted,
  lineHeight: 1.5,
};

const emptyStateStyle: React.CSSProperties = {
  padding: `${spacing.md}px ${spacing.sm}px`,
  fontSize: 11,
  color: colors.textMuted,
  textAlign: "center",
  background: colors.bgSoft,
  borderRadius: radius.card,
  border: `1px dashed ${colors.border}`,
};

const listStyle: React.CSSProperties = {
  listStyle: "none",
  margin: 0,
  padding: 0,
  display: "flex",
  flexDirection: "column",
  gap: spacing.xs,
};

const itemStyle: React.CSSProperties = {
  background: colors.bgSoft,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
  padding: spacing.sm,
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  gap: spacing.sm,
};

const itemHeaderStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: spacing.xs,
  flex: 1,
  minWidth: 0,
};

const pillStyle: React.CSSProperties = {
  background: colors.accent,
  color: colors.textInverse,
  fontSize: 10,
  fontWeight: 600,
  padding: "2px 8px",
  borderRadius: radius.pill,
  textTransform: "uppercase",
  letterSpacing: 0.4,
};

const actionTextStyle: React.CSSProperties = {
  fontSize: 11,
  color: colors.text,
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
};

const addCardStyle: React.CSSProperties = {
  marginTop: spacing.sm,
  background: colors.bgSoft,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card,
  padding: spacing.sm,
  display: "flex",
  flexDirection: "column",
  gap: spacing.xs,
};

const cardHeaderStyle: React.CSSProperties = {
  fontSize: 12,
  color: colors.text,
  marginBottom: spacing.xs,
};

const fieldStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 2,
};

const labelTextStyle: React.CSSProperties = {
  fontSize: 10,
  color: colors.textMuted,
  textTransform: "uppercase",
  letterSpacing: 0.4,
};

const selectStyle: React.CSSProperties = {
  padding: "6px 8px",
  fontSize: 12,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card / 2,
  background: colors.bg,
  color: colors.text,
};

function removeBtn(disabled: boolean): React.CSSProperties {
  return {
    padding: "4px 10px",
    fontSize: 11,
    fontWeight: 500,
    background: colors.bg,
    color: disabled ? colors.textMuted : "#dc2626",
    border: `1px solid ${disabled ? colors.border : "#dc2626"}`,
    borderRadius: radius.pill,
    cursor: disabled ? "not-allowed" : "pointer",
  };
}

function primaryBtn(disabled: boolean): React.CSSProperties {
  return {
    padding: "8px 14px",
    fontSize: 12,
    fontWeight: 600,
    background: disabled ? colors.bgSoft : colors.accent,
    color: disabled ? colors.textMuted : colors.textInverse,
    border: `1px solid ${disabled ? colors.border : colors.accent}`,
    borderRadius: radius.pill,
    cursor: disabled ? "not-allowed" : "pointer",
    marginTop: spacing.xs,
  };
}

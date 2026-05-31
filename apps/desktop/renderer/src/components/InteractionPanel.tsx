// InteractionPanel — Phase 1 Block A Task 3, extended in Phase 11
// Block C Tasks 13/15/17.
//
// Prototype-mode right-panel face. Lists interactions attached to the
// currently selected node and lets the user add new triggers
// (click / hover / press / mouse_enter / mouse_leave / after_delay)
// and actions (navigate_to / scroll_to / open_overlay / close_overlay
// / back / switch_variant). Each animation-capable action carries a
// `Transition` config (animation, duration, easing curve, optional
// slide direction). Backed by the `window.kcreate.interaction` bridge.
//
// Real implementation — there is no mock data, no stub. Every render
// reflects the live bridge state for the selected node.

import { useCallback, useEffect, useMemo, useState } from "react";

import type {
  AnimationType,
  EasingCurve,
  Interaction,
  InteractionAction,
  InteractionTrigger,
  NodeInfo,
  SlideDirection,
  Transition,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";
import { errorMessage } from "../lib/errorMessage";

export interface InteractionPanelProps {
  selected: NodeInfo | null;
  artboards: Array<{ id: string; name: string }>;
  /**
   * Full document tree, used by the `scroll_to` target picker. When
   * empty (or omitted), the panel still lets the user add
   * `navigate_to` / `open_overlay` / `close_overlay` / `back` /
   * `switch_variant` interactions but disables `scroll_to` because
   * there is no node catalog to pick from.
   */
  tree?: NodeInfo[];
  /**
   * Component variants visible to the `switch_variant` action.
   * When the selected node is a component instance, this list
   * holds the sibling variants of the instance's source
   * component. Empty when the node is not an instance or the
   * component has only one variant.
   */
  variants?: Array<{ id: string; name: string }>;
  onStatus?: (msg: string | null) => void;
  /** Called after every successful add / remove. The host typically
   * refreshes the document tree so lightning-bolt indicators in the
   * layer panel stay accurate. */
  onChanged?: () => void;
}

/** Discriminator used to drive the `<select>` for trigger choice. */
type TriggerKind =
  | "click"
  | "hover"
  | "press"
  | "mouse_enter"
  | "mouse_leave"
  | "after_delay";

const TRIGGER_KINDS: ReadonlyArray<{ id: TriggerKind; label: string }> = [
  { id: "click", label: "Click" },
  { id: "hover", label: "Hover" },
  { id: "press", label: "Press" },
  { id: "mouse_enter", label: "Mouse enter" },
  { id: "mouse_leave", label: "Mouse leave" },
  { id: "after_delay", label: "After delay" },
];

type ActionKind = InteractionAction["kind"];

const ACTION_KINDS: ReadonlyArray<{ id: ActionKind; label: string }> = [
  { id: "navigate_to", label: "Navigate to artboard" },
  { id: "scroll_to", label: "Scroll to node" },
  { id: "open_overlay", label: "Open overlay" },
  { id: "close_overlay", label: "Close overlay" },
  { id: "back", label: "Back" },
  { id: "switch_variant", label: "Switch component variant" },
];

const ANIMATION_KINDS: ReadonlyArray<{ id: AnimationType; label: string }> = [
  { id: "instant", label: "Instant" },
  { id: "dissolve", label: "Dissolve" },
  { id: "slide_in", label: "Slide in" },
  { id: "slide_out", label: "Slide out" },
  { id: "push", label: "Push" },
  { id: "move_in", label: "Move in" },
];

type EasingKind = EasingCurve["kind"];

const EASING_KINDS: ReadonlyArray<{ id: EasingKind; label: string }> = [
  { id: "linear", label: "Linear" },
  { id: "ease_in", label: "Ease in" },
  { id: "ease_out", label: "Ease out" },
  { id: "ease_in_out", label: "Ease in / out" },
  { id: "spring", label: "Spring" },
];

const SLIDE_DIRECTIONS: ReadonlyArray<{ id: SlideDirection; label: string }> = [
  { id: "left", label: "Left" },
  { id: "right", label: "Right" },
  { id: "up", label: "Up" },
  { id: "down", label: "Down" },
];

/** Actions that carry a `Transition` field in the wire format. */
const ANIMATED_KINDS: ReadonlySet<ActionKind> = new Set([
  "navigate_to",
  "open_overlay",
  "switch_variant",
]);

/** Animations that consume a `direction` parameter. */
const DIRECTIONAL_ANIMATIONS: ReadonlySet<AnimationType> = new Set([
  "slide_in",
  "slide_out",
  "push",
  "move_in",
]);

export function InteractionPanel({
  selected,
  artboards,
  tree,
  variants,
  onStatus,
  onChanged,
}: InteractionPanelProps): JSX.Element {
  const [items, setItems] = useState<Interaction[]>([]);
  const [triggerKind, setTriggerKind] = useState<TriggerKind>("click");
  const [delayMs, setDelayMs] = useState<number>(1500);
  const [actionKind, setActionKind] = useState<ActionKind>("navigate_to");
  const [targetId, setTargetId] = useState<string>("");
  const [animation, setAnimation] = useState<AnimationType>("instant");
  const [durationMs, setDurationMs] = useState<number>(300);
  const [easingKind, setEasingKind] = useState<EasingKind>("ease_in_out");
  const [springStiffness, setSpringStiffness] = useState<number>(180);
  const [springDamping, setSpringDamping] = useState<number>(20);
  const [direction, setDirection] = useState<SlideDirection>("left");
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

  // Pickable target nodes for `scroll_to`. Exclude the node the
  // interaction is attached to (scrolling to yourself is a no-op) and
  // anything without geometry. We don't filter to a specific node
  // type because any visible layer is a legitimate scroll target.
  const scrollTargets = useMemo<NodeInfo[]>(
    () =>
      (tree ?? []).filter(
        (n) =>
          n.id !== selected?.id &&
          n.visible &&
          n.bounds.width > 0 &&
          n.bounds.height > 0,
      ),
    [tree, selected?.id],
  );

  const variantTargets = useMemo<Array<{ id: string; name: string }>>(
    () => variants ?? [],
    [variants],
  );

  useEffect(() => {
    if (actionKind === "navigate_to" || actionKind === "open_overlay") {
      const first = artboards[0];
      const known = artboards.some((a) => a.id === targetId);
      if (!known) setTargetId(first ? first.id : "");
    } else if (actionKind === "scroll_to") {
      const first = scrollTargets[0];
      const known = scrollTargets.some((n) => n.id === targetId);
      if (!known) setTargetId(first ? first.id : "");
    } else if (actionKind === "switch_variant") {
      const first = variantTargets[0];
      const known = variantTargets.some((v) => v.id === targetId);
      if (!known) setTargetId(first ? first.id : "");
    } else if (targetId !== "") {
      setTargetId("");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [actionKind, artboards, scrollTargets, variantTargets]);

  if (!selected) {
    return (
      <div style={emptyStateStyle}>
        Select a layer to manage its interactions.
      </div>
    );
  }

  const needsArtboardTarget =
    actionKind === "navigate_to" || actionKind === "open_overlay";
  const needsNodeTarget = actionKind === "scroll_to";
  const needsVariantTarget = actionKind === "switch_variant";
  const needsTarget = needsArtboardTarget || needsNodeTarget || needsVariantTarget;
  const supportsTransition = ANIMATED_KINDS.has(actionKind);
  const needsDirection =
    supportsTransition && DIRECTIONAL_ANIMATIONS.has(animation);
  const canAdd =
    !busy &&
    (!needsTarget || targetId !== "") &&
    (triggerKind !== "after_delay" || delayMs >= 0);

  const buildTrigger = (): InteractionTrigger => {
    if (triggerKind === "after_delay") {
      return { kind: "after_delay", ms: Math.max(0, Math.round(delayMs)) };
    }
    return triggerKind;
  };

  const buildTransition = (): Transition => {
    const easing: EasingCurve =
      easingKind === "spring"
        ? {
            kind: "spring",
            stiffness: Math.max(1, springStiffness),
            damping: Math.max(0, springDamping),
          }
        : { kind: easingKind as Exclude<EasingKind, "spring" | "cubic_bezier"> };
    return {
      animation,
      duration_ms: Math.max(0, Math.round(durationMs)),
      easing,
      direction: needsDirection ? direction : null,
    };
  };

  const handleAdd = async (): Promise<void> => {
    if (!canAdd) return;
    setBusy(true);
    try {
      const action = buildAction(actionKind, targetId, buildTransition());
      const trigger = buildTrigger();
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
                <span style={pillStyle}>{describeTrigger(it.trigger)}</span>
                <span style={actionTextStyle}>
                  {describeAction(it.action, artboards, variantTargets)}
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
            value={triggerKind}
            onChange={(e) => setTriggerKind(e.target.value as TriggerKind)}
            style={selectStyle}
          >
            {TRIGGER_KINDS.map((t) => (
              <option key={t.id} value={t.id}>
                {t.label}
              </option>
            ))}
          </select>
        </label>
        {triggerKind === "after_delay" ? (
          <label style={fieldStyle}>
            <span style={labelTextStyle}>Delay (ms)</span>
            <input
              type="number"
              min={0}
              step={50}
              value={delayMs}
              onChange={(e) => setDelayMs(Number(e.target.value))}
              style={selectStyle}
            />
          </label>
        ) : null}
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
        {needsArtboardTarget ? (
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
        {needsNodeTarget ? (
          <label style={fieldStyle}>
            <span style={labelTextStyle}>Target node</span>
            <select
              value={targetId}
              onChange={(e) => setTargetId(e.target.value)}
              style={selectStyle}
            >
              {scrollTargets.length === 0 ? (
                <option value="">— no scrollable nodes —</option>
              ) : (
                scrollTargets.map((n) => (
                  <option key={n.id} value={n.id}>
                    {n.name || `${n.nodeType} ${n.id.slice(0, 8)}`}
                  </option>
                ))
              )}
            </select>
          </label>
        ) : null}
        {needsVariantTarget ? (
          <label style={fieldStyle}>
            <span style={labelTextStyle}>Target variant</span>
            <select
              value={targetId}
              onChange={(e) => setTargetId(e.target.value)}
              style={selectStyle}
            >
              {variantTargets.length === 0 ? (
                <option value="">— select a component instance —</option>
              ) : (
                variantTargets.map((v) => (
                  <option key={v.id} value={v.id}>
                    {v.name}
                  </option>
                ))
              )}
            </select>
          </label>
        ) : null}
        {supportsTransition ? (
          <>
            <label style={fieldStyle}>
              <span style={labelTextStyle}>Animation</span>
              <select
                value={animation}
                onChange={(e) =>
                  setAnimation(e.target.value as AnimationType)
                }
                style={selectStyle}
              >
                {ANIMATION_KINDS.map((a) => (
                  <option key={a.id} value={a.id}>
                    {a.label}
                  </option>
                ))}
              </select>
            </label>
            {animation !== "instant" ? (
              <>
                <label style={fieldStyle}>
                  <span style={labelTextStyle}>Duration (ms)</span>
                  <input
                    type="number"
                    min={0}
                    step={25}
                    value={durationMs}
                    onChange={(e) => setDurationMs(Number(e.target.value))}
                    style={selectStyle}
                  />
                </label>
                <label style={fieldStyle}>
                  <span style={labelTextStyle}>Easing</span>
                  <select
                    value={easingKind}
                    onChange={(e) =>
                      setEasingKind(e.target.value as EasingKind)
                    }
                    style={selectStyle}
                  >
                    {EASING_KINDS.map((c) => (
                      <option key={c.id} value={c.id}>
                        {c.label}
                      </option>
                    ))}
                  </select>
                </label>
                {easingKind === "spring" ? (
                  <>
                    <label style={fieldStyle}>
                      <span style={labelTextStyle}>Spring stiffness</span>
                      <input
                        type="number"
                        min={1}
                        step={10}
                        value={springStiffness}
                        onChange={(e) =>
                          setSpringStiffness(Number(e.target.value))
                        }
                        style={selectStyle}
                      />
                    </label>
                    <label style={fieldStyle}>
                      <span style={labelTextStyle}>Spring damping</span>
                      <input
                        type="number"
                        min={0}
                        step={1}
                        value={springDamping}
                        onChange={(e) =>
                          setSpringDamping(Number(e.target.value))
                        }
                        style={selectStyle}
                      />
                    </label>
                  </>
                ) : null}
                {needsDirection ? (
                  <label style={fieldStyle}>
                    <span style={labelTextStyle}>Direction</span>
                    <select
                      value={direction}
                      onChange={(e) =>
                        setDirection(e.target.value as SlideDirection)
                      }
                      style={selectStyle}
                    >
                      {SLIDE_DIRECTIONS.map((d) => (
                        <option key={d.id} value={d.id}>
                          {d.label}
                        </option>
                      ))}
                    </select>
                  </label>
                ) : null}
              </>
            ) : null}
          </>
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

function buildAction(
  kind: ActionKind,
  targetId: string,
  transition: Transition,
): InteractionAction {
  switch (kind) {
    case "navigate_to":
      return { kind: "navigate_to", target_artboard_id: targetId, transition };
    case "scroll_to":
      return { kind: "scroll_to", target_node_id: targetId };
    case "open_overlay":
      return {
        kind: "open_overlay",
        overlay_artboard_id: targetId,
        transition,
      };
    case "close_overlay":
      return { kind: "close_overlay" };
    case "back":
      return { kind: "back" };
    case "switch_variant":
      return { kind: "switch_variant", variant_id: targetId, transition };
    default: {
      // Exhaustiveness sentinel — TS yells if a new action kind is
      // added to the wire format that we forgot to handle here.
      const _exhaustive: never = kind;
      throw new Error(`unsupported action kind: ${String(_exhaustive)}`);
    }
  }
}

function describeTrigger(t: InteractionTrigger): string {
  if (typeof t === "string") return t;
  if (t.kind === "after_delay") return `after ${t.ms} ms`;
  // Forward-compat: render unknown object triggers by their kind.
  return (t as { kind: string }).kind;
}

function describeAction(
  action: InteractionAction,
  artboards: Array<{ id: string; name: string }>,
  variants: Array<{ id: string; name: string }>,
): string {
  const lookupArt = (id: string): string => {
    const ab = artboards.find((a) => a.id === id);
    return ab ? ab.name : `${id.slice(0, 8)}…`;
  };
  const lookupVar = (id: string): string => {
    const v = variants.find((x) => x.id === id);
    return v ? v.name : `${id.slice(0, 8)}…`;
  };
  switch (action.kind) {
    case "navigate_to":
      return `Navigate to ${lookupArt(action.target_artboard_id)}`;
    case "scroll_to":
      return `Scroll to ${action.target_node_id.slice(0, 8)}…`;
    case "open_overlay":
      return `Open overlay ${lookupArt(action.overlay_artboard_id)}`;
    case "close_overlay":
      return "Close overlay";
    case "back":
      return "Back";
    case "switch_variant":
      return `Switch variant → ${lookupVar(action.variant_id)}`;
    default: {
      const _exhaustive: never = action;
      return String(_exhaustive);
    }
  }
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
    width: "100%",
    padding: "8px 12px",
    fontSize: 12,
    fontWeight: 600,
    background: disabled ? colors.bgSoft : colors.accent,
    color: disabled ? colors.textMuted : colors.textInverse,
    border: `1px solid ${disabled ? colors.border : colors.accent}`,
    borderRadius: radius.pill,
    cursor: disabled ? "not-allowed" : "pointer",
    marginTop: spacing.sm,
  };
}

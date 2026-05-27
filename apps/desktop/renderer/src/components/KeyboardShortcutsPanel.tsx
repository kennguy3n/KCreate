// KeyboardShortcutsPanel — Phase 6 (Tasks 21–22).
//
// Lists every shortcut action exposed by the registry and lets the
// user rebind any of them. Captures the next keystroke in a
// modal-style "recording" input so the user can press the exact key
// combination they want without typing it textually.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  ACTION_META,
  type ActionId,
  type ShortcutBinding,
  type ShortcutCategory,
  bindingsEqual,
  formatBinding,
  shortcutStore,
} from "../shortcuts/registry";
import { useShortcutBindings } from "../shortcuts/useShortcuts";
import { colors, radius, spacing } from "../styles/tokens";

const CATEGORIES: { id: ShortcutCategory; title: string }[] = [
  { id: "editing", title: "Editing" },
  { id: "tools", title: "Tools" },
  { id: "view", title: "View" },
  { id: "panels", title: "Panels" },
];

export interface KeyboardShortcutsPanelProps {
  onStatus?: (msg: string | null) => void;
}

export function KeyboardShortcutsPanel({
  onStatus,
}: KeyboardShortcutsPanelProps) {
  const bindings = useShortcutBindings();
  const [recording, setRecording] = useState<ActionId | null>(null);
  const [filter, setFilter] = useState("");

  const grouped = useMemo(() => {
    const out = new Map<ShortcutCategory, ActionId[]>();
    for (const id of Object.keys(ACTION_META) as ActionId[]) {
      const meta = ACTION_META[id];
      if (filter) {
        const needle = filter.toLowerCase();
        const haystack = `${meta.label} ${meta.description}`.toLowerCase();
        if (!haystack.includes(needle)) continue;
      }
      const list = out.get(meta.category) ?? [];
      list.push(id);
      out.set(meta.category, list);
    }
    return out;
  }, [filter]);

  // Precompute the conflict map once per bindings snapshot:
  // `conflicts.get(id)` lists every *other* action currently bound to
  // the same keystroke. Rendering this in the panel makes the
  // first-match-wins dispatch order in `useShortcuts` visible to the
  // user — without it, a binding could be silently shadowed.
  //
  // Pairwise iteration is O(N^2) over the action set, but N ≤ 20 so
  // this is a few hundred comparisons per render, well under any
  // perceptible budget. We avoid a hash-based index because the
  // binding key uses a small, structured tuple (key + 3 booleans);
  // a real index would just be reimplementing equality with extra
  // ceremony.
  const conflictMap = useMemo(() => {
    const out = new Map<ActionId, ActionId[]>();
    const ids = Object.keys(ACTION_META) as ActionId[];
    for (const id of ids) {
      const mine = bindings[id];
      const peers: ActionId[] = [];
      for (const other of ids) {
        if (other === id) continue;
        if (bindingsEqual(mine, bindings[other])) {
          peers.push(other);
        }
      }
      if (peers.length > 0) out.set(id, peers);
    }
    return out;
  }, [bindings]);

  const handleRebind = useCallback(
    (id: ActionId, binding: ShortcutBinding) => {
      shortcutStore().set(id, binding);
      setRecording(null);
      onStatus?.(`Bound ${ACTION_META[id].label} to ${formatBinding(binding)}.`);
    },
    [onStatus],
  );

  const handleReset = useCallback(
    (id: ActionId) => {
      shortcutStore().resetOne(id);
      onStatus?.(`Restored default binding for ${ACTION_META[id].label}.`);
    },
    [onStatus],
  );

  const handleResetAll = useCallback(() => {
    shortcutStore().resetAll();
    onStatus?.("Restored all shortcuts to their shipped defaults.");
  }, [onStatus]);

  return (
    <div
      data-testid="kc-shortcuts-panel"
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.md,
        padding: spacing.md,
        background: colors.bg,
        borderRadius: radius.card,
        color: colors.text,
      }}
    >
      <header
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
        }}
      >
        <div>
          <h2 style={{ margin: 0, fontSize: 18 }}>Keyboard shortcuts</h2>
          <p style={{ margin: 0, color: colors.textMuted, fontSize: 13 }}>
            Click a binding to rebind. Press the new keystroke; press
            Escape to cancel.
          </p>
        </div>
        <button
          type="button"
          onClick={handleResetAll}
          style={{
            background: "transparent",
            border: `1px solid ${colors.border}`,
            borderRadius: radius.sm,
            padding: `${spacing.xs}px ${spacing.sm}px`,
            color: colors.text,
            cursor: "pointer",
          }}
        >
          Restore all defaults
        </button>
      </header>

      <input
        type="search"
        value={filter}
        onChange={(e) => setFilter(e.target.value)}
        placeholder="Filter shortcuts"
        aria-label="Filter shortcuts"
        style={{
          padding: `${spacing.xs}px ${spacing.sm}px`,
          border: `1px solid ${colors.border}`,
          borderRadius: radius.sm,
          fontSize: 13,
        }}
      />

      {CATEGORIES.map((cat) => {
        const ids = grouped.get(cat.id);
        if (!ids || ids.length === 0) return null;
        return (
          <section
            key={cat.id}
            style={{
              border: `1px solid ${colors.border}`,
              borderRadius: radius.md,
              overflow: "hidden",
            }}
          >
            <header
              style={{
                background: colors.bgSoft,
                padding: `${spacing.xs}px ${spacing.sm}px`,
                fontWeight: 600,
                fontSize: 13,
              }}
            >
              {cat.title}
            </header>
            <ul
              style={{
                listStyle: "none",
                margin: 0,
                padding: 0,
              }}
            >
              {ids.map((id) => (
                <ShortcutRow
                  key={id}
                  id={id}
                  binding={bindings[id]}
                  conflicts={conflictMap.get(id) ?? null}
                  recording={recording === id}
                  onStartRecording={() => setRecording(id)}
                  onCancelRecording={() => setRecording(null)}
                  onRebind={handleRebind}
                  onReset={handleReset}
                />
              ))}
            </ul>
          </section>
        );
      })}
    </div>
  );
}

interface ShortcutRowProps {
  id: ActionId;
  binding: ShortcutBinding;
  /// Other actions currently bound to the same keystroke, or `null`
  /// if there is no collision. When non-null, the row renders a
  /// warning line listing the conflicting actions so the user can
  /// resolve the collision (rebind one of them) — first-match-wins
  /// dispatch means a collision silently shadows whichever action
  /// loses the iteration race in `useShortcuts`.
  conflicts: ActionId[] | null;
  recording: boolean;
  onStartRecording: () => void;
  onCancelRecording: () => void;
  onRebind: (id: ActionId, binding: ShortcutBinding) => void;
  onReset: (id: ActionId) => void;
}

function ShortcutRow({
  id,
  binding,
  conflicts,
  recording,
  onStartRecording,
  onCancelRecording,
  onRebind,
  onReset,
}: ShortcutRowProps) {
  const meta = ACTION_META[id];
  const rowRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!recording) return;
    const onKeyDown = (event: KeyboardEvent): void => {
      event.preventDefault();
      event.stopPropagation();
      // Allow the user to cancel without committing a useless
      // binding. Modifier-only presses are also ignored — wait
      // for a "real" key.
      if (event.key === "Escape") {
        onCancelRecording();
        return;
      }
      if (
        event.key === "Control" ||
        event.key === "Meta" ||
        event.key === "Shift" ||
        event.key === "Alt"
      ) {
        return;
      }
      const next: ShortcutBinding = {
        key: event.key.length === 1 ? event.key.toLowerCase() : event.key,
        mod: event.ctrlKey || event.metaKey,
        shift: event.shiftKey,
        alt: event.altKey,
      };
      onRebind(id, next);
    };
    window.addEventListener("keydown", onKeyDown, { capture: true });
    return () =>
      window.removeEventListener("keydown", onKeyDown, { capture: true });
  }, [recording, id, onRebind, onCancelRecording]);

  // Human-readable list of conflicting action labels for the
  // warning line and the row's accessible label.
  const conflictLabels =
    conflicts && conflicts.length > 0
      ? conflicts.map((c) => ACTION_META[c].label).join(", ")
      : null;

  return (
    <li
      data-testid={`kc-shortcut-row-${id}`}
      data-has-conflict={conflictLabels ? "true" : "false"}
      style={{
        display: "flex",
        flexDirection: "column",
        padding: `${spacing.xs}px ${spacing.sm}px`,
        borderTop: `1px solid ${colors.border}`,
        gap: spacing.xs,
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: spacing.sm,
        }}
      >
        <div ref={rowRef} style={{ flex: 1, minWidth: 0 }}>
          <div style={{ fontSize: 13, fontWeight: 500 }}>{meta.label}</div>
          <div style={{ fontSize: 12, color: colors.textMuted }}>
            {meta.description}
          </div>
        </div>
        <button
          type="button"
          onClick={recording ? onCancelRecording : onStartRecording}
          aria-pressed={recording}
          aria-label={
            recording
              ? `Recording new binding for ${meta.label}. Press Escape to cancel.`
              : conflictLabels
                ? `Current binding for ${meta.label}: ${formatBinding(binding)}. Conflicts with ${conflictLabels}. Click to rebind.`
                : `Current binding for ${meta.label}: ${formatBinding(binding)}. Click to rebind.`
          }
          style={{
            minWidth: 90,
            padding: `${spacing.xs}px ${spacing.sm}px`,
            background: recording ? colors.accent : "transparent",
            color: recording ? colors.textInverse : colors.text,
            border: `1px solid ${
              recording
                ? colors.accent
                : conflictLabels
                  ? colors.warn
                  : colors.border
            }`,
            borderRadius: radius.sm,
            fontFamily: "monospace",
            fontSize: 12,
            cursor: "pointer",
          }}
        >
          {recording ? "Press a key…" : formatBinding(binding)}
        </button>
        <button
          type="button"
          onClick={() => onReset(id)}
          aria-label={`Restore default for ${meta.label}`}
          style={{
            padding: `${spacing.xs}px ${spacing.sm}px`,
            background: "transparent",
            border: `1px solid ${colors.border}`,
            borderRadius: radius.sm,
            fontSize: 12,
            color: colors.textMuted,
            cursor: "pointer",
          }}
        >
          Default
        </button>
      </div>
      {conflictLabels ? (
        // Conflict warning. The dispatch loop in `useShortcuts`
        // fires whichever action it iterates to first; the user
        // can't predict that order, so the only safe UX is to
        // surface the collision and let them rebind. `role="alert"`
        // would be too noisy for a passive observation; we use the
        // visual warn-tinted strip + accessible label on the
        // rebind button (above) instead.
        <div
          data-testid={`kc-shortcut-conflict-${id}`}
          style={{
            fontSize: 12,
            color: colors.warn,
            paddingLeft: spacing.xs,
          }}
        >
          Also bound to {conflictLabels}. Only one action fires per
          keystroke; rebind one of them to clear the conflict.
        </div>
      ) : null}
    </li>
  );
}

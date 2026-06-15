// CommandPalette — H1 discoverability unlock.
//
// A fast, fuzzy-searchable overlay (Cmd/Ctrl+K) that lists every
// editor action, panel, and create flow and runs the REAL handler for
// each. It is intentionally *presentational + self-contained*: the
// host (EditorPage) composes the `commands` array — each command's
// `run` closes over the same handler the toolbar / shortcut already
// invokes, so there is exactly one implementation of every action and
// the palette can never drift out of sync with a "fake" menu item.
//
// Ranking:
//   * empty query  → grouped view, a "Recent" group (from persisted
//                     usage history) on top, then the host's groups in
//                     first-seen order.
//   * non-empty    → a single flat list ranked by fuzzy score plus a
//                     recency/frequency boost, best match first.
//
// Keyboard: ↑/↓ move, Enter runs, Esc closes, Cmd/Ctrl+K toggles. The
// search input keeps focus the whole time so typing never escapes to
// the global tool shortcuts (which `useShortcuts` gates on form-field
// focus anyway).

import { useEffect, useMemo, useRef, useState } from "react";

import type { ShortcutBinding } from "../shortcuts/registry";
import { formatBinding } from "../shortcuts/registry";
import {
  loadCommandHistory,
  recordCommandUse,
  type CommandHistory,
} from "../lib/commandPaletteHistory";
import { fuzzyScoreFields } from "../lib/fuzzyMatch";
import { colors, font, radius, spacing } from "../styles/tokens";
import { Icon, type IconName } from "./Icon";

/** A single palette entry. The host owns `run`. */
export interface PaletteCommand {
  /** Stable id — used for React keys and usage-history boosting. */
  readonly id: string;
  /** Visible label, e.g. "Start from a template". */
  readonly label: string;
  /** Group header shown in the empty-query grouped view. */
  readonly group: string;
  /** Optional leading glyph. */
  readonly icon?: IconName;
  /** Extra terms folded into the fuzzy match (synonyms, e.g. "ai"
   *  for "Generate with AI"). Never rendered. */
  readonly keywords?: readonly string[];
  /** Optional shortcut hint rendered on the right. */
  readonly shortcut?: ShortcutBinding | null;
  /** When true the row is shown greyed-out and cannot be run (used to
   *  keep a capability discoverable while explaining why it's
   *  unavailable right now). */
  readonly disabled?: boolean;
  /** One-line reason shown when `disabled`. */
  readonly disabledReason?: string;
  /** The real handler. Invoked on Enter / click. */
  readonly run: () => void;
}

export interface CommandPaletteProps {
  /** Parent owns visibility (toggled by the openCommandPalette action). */
  readonly open: boolean;
  /** Full command set; the palette filters + ranks it. */
  readonly commands: readonly PaletteCommand[];
  /** Dismiss the palette. */
  readonly onClose: () => void;
}

const RECENT_LIMIT = 6;
const RECENT_GROUP = "Recent";

interface ScoredCommand {
  readonly command: PaletteCommand;
  readonly score: number;
  /** Indices into `command.label` to highlight (label matches only). */
  readonly indices: readonly number[];
}

type Row =
  | { readonly kind: "header"; readonly label: string }
  | {
      readonly kind: "command";
      readonly key: string;
      readonly command: PaletteCommand;
      readonly indices: readonly number[];
    };

/** Stable per-(group,command) row key — a command can appear both in
 *  "Recent" and in its own group, so the id alone isn't unique. */
function rowKey(group: string, id: string): string {
  return `${group}\u0000${id}`;
}

/**
 * Build the visible row list for the current query. Pure so it can be
 * unit-tested and memoised. `now` is injected for deterministic
 * recency boosts in tests.
 */
export function buildRows(
  commands: readonly PaletteCommand[],
  query: string,
  history: CommandHistory,
  now: number = Date.now(),
): Row[] {
  const trimmed = query.trim().toLowerCase();

  if (trimmed.length === 0) {
    const rows: Row[] = [];
    const byId = new Map(commands.map((c) => [c.id, c]));

    // Recent group: history ids that still exist as commands.
    const recent = history
      .recentIds()
      .map((id) => byId.get(id))
      .filter((c): c is PaletteCommand => c !== undefined)
      .slice(0, RECENT_LIMIT);
    if (recent.length > 0) {
      rows.push({ kind: "header", label: RECENT_GROUP });
      for (const command of recent) {
        rows.push({
          kind: "command",
          key: rowKey(RECENT_GROUP, command.id),
          command,
          indices: [],
        });
      }
    }

    // Then every command grouped by `group`, groups in first-seen
    // order (preserves the host's intentional ordering).
    const order: string[] = [];
    const grouped = new Map<string, PaletteCommand[]>();
    for (const command of commands) {
      let bucket = grouped.get(command.group);
      if (bucket === undefined) {
        bucket = [];
        grouped.set(command.group, bucket);
        order.push(command.group);
      }
      bucket.push(command);
    }
    for (const group of order) {
      rows.push({ kind: "header", label: group });
      for (const command of grouped.get(group) ?? []) {
        rows.push({
          kind: "command",
          key: rowKey(group, command.id),
          command,
          indices: [],
        });
      }
    }
    return rows;
  }

  // Non-empty query: flat ranked list.
  const scored: ScoredCommand[] = [];
  for (const command of commands) {
    const fields = [command.label, ...(command.keywords ?? [])];
    const match = fuzzyScoreFields(fields, trimmed);
    if (match === null) continue;
    scored.push({
      command,
      score: match.score + history.boost(command.id, now),
      indices: match.indices,
    });
  }
  scored.sort((a, b) => {
    if (b.score !== a.score) return b.score - a.score;
    return a.command.label.localeCompare(b.command.label);
  });
  return scored.map((s) => ({
    kind: "command" as const,
    key: rowKey("results", s.command.id),
    command: s.command,
    indices: s.indices,
  }));
}

/** Split a label into highlighted / plain spans for the matched indices. */
function HighlightedLabel({
  label,
  indices,
}: {
  label: string;
  indices: readonly number[];
}): JSX.Element {
  if (indices.length === 0) return <>{label}</>;
  const hit = new Set(indices);
  const spans: JSX.Element[] = [];
  let run = "";
  let runHighlighted = hit.has(0);
  const flush = (key: number): void => {
    if (run.length === 0) return;
    spans.push(
      runHighlighted ? (
        <mark
          key={key}
          style={{
            background: "transparent",
            color: colors.accent,
            fontWeight: 700,
          }}
        >
          {run}
        </mark>
      ) : (
        <span key={key}>{run}</span>
      ),
    );
    run = "";
  };
  for (let i = 0; i < label.length; i += 1) {
    const isHit = hit.has(i);
    if (isHit !== runHighlighted) {
      flush(i);
      runHighlighted = isHit;
    }
    run += label[i];
  }
  flush(label.length);
  return <>{spans}</>;
}

export function CommandPalette({
  open,
  commands,
  onClose,
}: CommandPaletteProps): JSX.Element | null {
  const [query, setQuery] = useState("");
  const [activeKey, setActiveKey] = useState<string | null>(null);
  // Usage history is read once per open and bumped on each run; held
  // in state so a run re-ranks the "Recent" group live.
  const [history, setHistory] = useState<CommandHistory>(() =>
    loadCommandHistory(),
  );
  const inputRef = useRef<HTMLInputElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  const activeRowRef = useRef<HTMLButtonElement | null>(null);

  // Reset transient state and refresh history each time the palette
  // opens so it reflects usage from the current session.
  useEffect(() => {
    if (!open) return;
    setQuery("");
    setHistory(loadCommandHistory());
    // Focus after the dialog has painted so the caret lands in the
    // input rather than the element that had focus when Ctrl+K fired.
    const id = window.requestAnimationFrame(() => inputRef.current?.focus());
    return () => window.cancelAnimationFrame(id);
  }, [open]);

  const rows = useMemo(
    () => buildRows(commands, query, history),
    [commands, query, history],
  );

  const commandKeys = useMemo(
    () => rows.filter((r) => r.kind === "command").map((r) => r.key),
    [rows],
  );

  // Keep the active row valid as the filtered set changes: snap to the
  // first command whenever the current active key drops out.
  useEffect(() => {
    if (activeKey !== null && commandKeys.includes(activeKey)) return;
    setActiveKey(commandKeys[0] ?? null);
  }, [commandKeys, activeKey]);

  // Scroll the active row into view on keyboard navigation.
  useEffect(() => {
    activeRowRef.current?.scrollIntoView({ block: "nearest" });
  }, [activeKey]);

  if (!open) return null;

  const runCommand = (command: PaletteCommand): void => {
    if (command.disabled) return;
    // Record usage BEFORE running: the handler may unmount the palette
    // (route change), and we want the boost persisted regardless.
    setHistory(recordCommandUse(command.id));
    onClose();
    command.run();
  };

  const moveActive = (delta: number): void => {
    if (commandKeys.length === 0) return;
    const current = activeKey === null ? -1 : commandKeys.indexOf(activeKey);
    const next =
      (current + delta + commandKeys.length) % commandKeys.length;
    setActiveKey(commandKeys[next] ?? null);
  };

  const onInputKeyDown = (
    event: React.KeyboardEvent<HTMLInputElement>,
  ): void => {
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        moveActive(1);
        break;
      case "ArrowUp":
        event.preventDefault();
        moveActive(-1);
        break;
      case "Enter": {
        event.preventDefault();
        const active = rows.find(
          (r) => r.kind === "command" && r.key === activeKey,
        );
        if (active && active.kind === "command") runCommand(active.command);
        break;
      }
      case "Escape":
        event.preventDefault();
        onClose();
        break;
      case "k":
      case "K":
        // Toggle-close: the global Ctrl+K is gated while the input has
        // focus, so honour the same chord here.
        if (event.metaKey || event.ctrlKey) {
          event.preventDefault();
          onClose();
        }
        break;
      default:
        break;
    }
  };

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Command palette"
      data-testid="kcreate-command-palette"
      style={overlayStyle}
      onClick={onClose}
    >
      <div style={paletteStyle} onClick={(e) => e.stopPropagation()}>
        <div style={inputRowStyle}>
          <Icon name="search" size={16} />
          <input
            ref={inputRef}
            type="text"
            value={query}
            placeholder="Search actions, panels, tools…"
            aria-label="Search commands"
            data-testid="kcreate-command-palette-input"
            spellCheck={false}
            autoComplete="off"
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onInputKeyDown}
            style={inputStyle}
          />
          <kbd style={escHintStyle}>Esc</kbd>
        </div>
        <div ref={listRef} role="listbox" style={listStyle}>
          {commandKeys.length === 0 ? (
            <div style={emptyStyle}>No matching commands.</div>
          ) : (
            rows.map((row) =>
              row.kind === "header" ? (
                <div key={`h:${row.label}`} style={headerStyle}>
                  {row.label}
                </div>
              ) : (
                <CommandRow
                  key={row.key}
                  command={row.command}
                  indices={row.indices}
                  active={row.key === activeKey}
                  activeRef={row.key === activeKey ? activeRowRef : null}
                  onHover={() => setActiveKey(row.key)}
                  onRun={() => runCommand(row.command)}
                />
              ),
            )
          )}
        </div>
        <div style={footerStyle}>
          <span>
            <kbd style={footKbd}>↑</kbd>
            <kbd style={footKbd}>↓</kbd> navigate
          </span>
          <span>
            <kbd style={footKbd}>↵</kbd> run
          </span>
          <span>
            <kbd style={footKbd}>esc</kbd> dismiss
          </span>
        </div>
      </div>
    </div>
  );
}

interface CommandRowProps {
  command: PaletteCommand;
  indices: readonly number[];
  active: boolean;
  activeRef: React.Ref<HTMLButtonElement> | null;
  onHover: () => void;
  onRun: () => void;
}

function CommandRow({
  command,
  indices,
  active,
  activeRef,
  onHover,
  onRun,
}: CommandRowProps): JSX.Element {
  const disabled = command.disabled ?? false;
  return (
    <button
      type="button"
      role="option"
      aria-selected={active}
      aria-disabled={disabled}
      data-testid={`kcreate-command-row-${command.id}`}
      ref={activeRef}
      // Hovering moves the active highlight so mouse + keyboard share
      // one selection model. Use `onMouseMove` (not `onMouseEnter`) so
      // the row under a stationary cursor re-activates after keyboard
      // nav moved the highlight away.
      onMouseMove={onHover}
      onClick={onRun}
      style={{
        ...rowStyle,
        background: active ? colors.bgSoft : "transparent",
        cursor: disabled ? "default" : "pointer",
        opacity: disabled ? 0.5 : 1,
      }}
    >
      {command.icon ? (
        <span style={rowIconStyle}>
          <Icon name={command.icon} size={16} />
        </span>
      ) : (
        <span style={rowIconStyle} />
      )}
      <span style={rowLabelStyle}>
        <HighlightedLabel label={command.label} indices={indices} />
        {disabled && command.disabledReason ? (
          <span style={rowReasonStyle}>{command.disabledReason}</span>
        ) : null}
      </span>
      {command.shortcut ? (
        <kbd style={rowShortcutStyle}>{formatBinding(command.shortcut)}</kbd>
      ) : null}
    </button>
  );
}

// ---------------------------------------------------------------------
// Styles
// ---------------------------------------------------------------------

const overlayStyle: React.CSSProperties = {
  position: "fixed",
  inset: 0,
  background: "rgba(17, 24, 39, 0.45)",
  display: "flex",
  alignItems: "flex-start",
  justifyContent: "center",
  paddingTop: "12vh",
  zIndex: 2500,
  fontFamily: font.family,
};

const paletteStyle: React.CSSProperties = {
  width: "min(640px, 92vw)",
  maxHeight: "70vh",
  background: colors.bg,
  borderRadius: radius.card,
  border: `1px solid ${colors.border}`,
  boxShadow: "0 24px 64px rgba(0,0,0,0.28)",
  display: "flex",
  flexDirection: "column",
  overflow: "hidden",
};

const inputRowStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: spacing.sm,
  padding: `${spacing.md}px ${spacing.md}px`,
  borderBottom: `1px solid ${colors.border}`,
  color: colors.textMuted,
};

const inputStyle: React.CSSProperties = {
  flex: 1,
  border: "none",
  outline: "none",
  background: "transparent",
  fontSize: 15,
  color: colors.text,
  fontFamily: font.family,
};

const escHintStyle: React.CSSProperties = {
  fontSize: 11,
  color: colors.textMuted,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  padding: "1px 6px",
};

const listStyle: React.CSSProperties = {
  flex: 1,
  overflowY: "auto",
  padding: spacing.xs,
};

const headerStyle: React.CSSProperties = {
  fontSize: 11,
  fontWeight: 600,
  textTransform: "uppercase",
  letterSpacing: 0.5,
  color: colors.textMuted,
  padding: `${spacing.sm}px ${spacing.sm}px ${spacing.xs}px`,
};

const rowStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: spacing.sm,
  width: "100%",
  textAlign: "left",
  border: "none",
  borderRadius: radius.md,
  padding: `${spacing.sm}px ${spacing.sm}px`,
  color: colors.text,
  fontSize: 14,
  fontFamily: font.family,
};

const rowIconStyle: React.CSSProperties = {
  display: "inline-flex",
  width: 18,
  justifyContent: "center",
  color: colors.textMuted,
  flexShrink: 0,
};

const rowLabelStyle: React.CSSProperties = {
  flex: 1,
  display: "flex",
  flexDirection: "column",
  minWidth: 0,
};

const rowReasonStyle: React.CSSProperties = {
  fontSize: 11,
  color: colors.textMuted,
};

const rowShortcutStyle: React.CSSProperties = {
  fontSize: 11,
  color: colors.textMuted,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  padding: "1px 6px",
  whiteSpace: "nowrap",
  flexShrink: 0,
};

const footerStyle: React.CSSProperties = {
  display: "flex",
  gap: spacing.md,
  padding: `${spacing.sm}px ${spacing.md}px`,
  borderTop: `1px solid ${colors.border}`,
  fontSize: 11,
  color: colors.textMuted,
};

const footKbd: React.CSSProperties = {
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  padding: "0 4px",
  marginRight: 2,
};

const emptyStyle: React.CSSProperties = {
  padding: `${spacing.lg}px`,
  textAlign: "center",
  color: colors.textMuted,
  fontSize: 14,
};

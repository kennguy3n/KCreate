// ConstraintsPanel — Phase 8 Block C Task 20.
//
// Edit the horizontal + vertical resize constraints (Fixed / Min /
// Max / Center / Scale / Stretch) on the selected node so that a
// parent frame's `Phase8Bridge.resizeFrame` recomputes child bounds
// per the constraint rules.
//
// Wires `window.kcreate.phase8.nodeConstraints` (read) +
// `setNodeConstraints` (write). Both surfaces are no-op when no
// node is selected.
//
// Each constraint is a 6-mode enum mirroring the Rust
// `kcreate_core::node::Constraint` definition. The dropdown order
// matches the Rust enum order so the muscle memory carries between
// the bridge tests and the UI.

import { useCallback, useEffect, useMemo, useState } from "react";

import type { Constraint, Constraints } from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface ConstraintsPanelProps {
  /** UUID of the node to edit. `null` collapses to a hint. */
  nodeId: string | null;
  /** Status sink. Same convention as PreflightPanel. */
  onStatus?: (msg: string | null) => void;
}

interface ConstraintOption {
  value: Constraint;
  label: string;
  description: string;
}

const CONSTRAINT_OPTIONS: ConstraintOption[] = [
  {
    value: "fixed",
    label: "Fixed",
    description:
      "Keep the same distance from the leading edge / top edge of the parent (axis-dependent).",
  },
  {
    value: "min",
    label: "Min",
    description:
      "Pin to the leading edge: x / y stays the same; width / height does not.",
  },
  {
    value: "max",
    label: "Max",
    description:
      "Pin to the trailing edge: the gap between the node and the parent's right / bottom edge is preserved.",
  },
  {
    value: "center",
    label: "Center",
    description:
      "Keep the node centred on the parent's centreline along this axis.",
  },
  {
    value: "scale",
    label: "Scale",
    description:
      "Scale position and size proportionally to the parent's resize ratio.",
  },
  {
    value: "stretch",
    label: "Stretch",
    description:
      "Pin both edges: leading + trailing gaps are preserved; width / height grows to fill.",
  },
];

const DEFAULT_CONSTRAINTS: Constraints = {
  horizontal: "fixed",
  vertical: "fixed",
};

export function ConstraintsPanel({
  nodeId,
  onStatus,
}: ConstraintsPanelProps): JSX.Element {
  const [constraints, setConstraints] = useState<Constraints>(
    DEFAULT_CONSTRAINTS,
  );
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Reload whenever the selected node changes. We do not re-fetch on
  // every parent resize because the local copy is the source of
  // truth between fetches — the bridge round-trip pattern is
  // pull-on-select, push-on-edit.
  useEffect(() => {
    if (nodeId == null) {
      setConstraints(DEFAULT_CONSTRAINTS);
      setLoaded(false);
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        const next = await window.kcreate.phase8.nodeConstraints(nodeId);
        if (cancelled) return;
        setConstraints(next);
        setLoaded(true);
      } catch (e) {
        if (cancelled) return;
        setError(`Load constraints: ${errMsg(e)}`);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [nodeId]);

  const persist = useCallback(
    async (next: Constraints): Promise<void> => {
      if (nodeId == null) return;
      setBusy(true);
      setError(null);
      try {
        await window.kcreate.phase8.setNodeConstraints(nodeId, next);
        onStatus?.(
          `Constraints set to ${next.horizontal} × ${next.vertical}.`,
        );
      } catch (e) {
        setError(`Save constraints: ${errMsg(e)}`);
      } finally {
        setBusy(false);
      }
    },
    [nodeId, onStatus],
  );

  const handleHorizontal = useCallback(
    async (value: Constraint): Promise<void> => {
      const next: Constraints = { ...constraints, horizontal: value };
      setConstraints(next);
      await persist(next);
    },
    [constraints, persist],
  );

  const handleVertical = useCallback(
    async (value: Constraint): Promise<void> => {
      const next: Constraints = { ...constraints, vertical: value };
      setConstraints(next);
      await persist(next);
    },
    [constraints, persist],
  );

  const horizontalDescription = useMemo(
    () =>
      CONSTRAINT_OPTIONS.find((o) => o.value === constraints.horizontal)
        ?.description ?? "",
    [constraints.horizontal],
  );
  const verticalDescription = useMemo(
    () =>
      CONSTRAINT_OPTIONS.find((o) => o.value === constraints.vertical)
        ?.description ?? "",
    [constraints.vertical],
  );

  if (nodeId == null) {
    return (
      <div
        style={{
          padding: spacing.md,
          fontSize: 12,
          color: colors.textMuted,
        }}
      >
        Select a node to edit its resize constraints.
      </div>
    );
  }

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.md,
        padding: spacing.md,
        fontSize: 12,
      }}
    >
      <header style={{ display: "flex", flexDirection: "column", gap: 2 }}>
        <h3 style={{ margin: 0, fontSize: 13, fontWeight: 600 }}>
          Constraints
        </h3>
        <small style={{ color: colors.textMuted }}>
          How this node behaves when its parent frame resizes. Applied
          on the next call to `resizeFrame`.
        </small>
      </header>

      <ConstraintVisualiser constraints={constraints} />

      <section style={{ display: "flex", flexDirection: "column", gap: 4 }}>
        <label style={fieldLabelStyle}>
          Horizontal
          <select
            value={constraints.horizontal}
            onChange={(e) => {
              void handleHorizontal(e.target.value as Constraint);
            }}
            style={selectStyle}
            disabled={busy || !loaded}
          >
            {CONSTRAINT_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </label>
        <small style={{ color: colors.textMuted, fontSize: 10 }}>
          {horizontalDescription}
        </small>
      </section>

      <section style={{ display: "flex", flexDirection: "column", gap: 4 }}>
        <label style={fieldLabelStyle}>
          Vertical
          <select
            value={constraints.vertical}
            onChange={(e) => {
              void handleVertical(e.target.value as Constraint);
            }}
            style={selectStyle}
            disabled={busy || !loaded}
          >
            {CONSTRAINT_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </label>
        <small style={{ color: colors.textMuted, fontSize: 10 }}>
          {verticalDescription}
        </small>
      </section>

      {error != null ? (
        <div
          style={{
            background: colors.dangerBgSoft,
            border: `1px solid ${colors.dangerBorder}`,
            color: colors.danger,
            padding: spacing.xs,
            borderRadius: radius.sm,
            display: "flex",
            justifyContent: "space-between",
            gap: 4,
            fontSize: 11,
          }}
        >
          <span>{error}</span>
          <button
            type="button"
            onClick={() => setError(null)}
            style={{
              background: "transparent",
              border: "none",
              color: colors.danger,
              cursor: "pointer",
              fontSize: 11,
            }}
            aria-label="Dismiss error"
          >
            ✕
          </button>
        </div>
      ) : null}
    </div>
  );
}

// A small SVG illustrating, schematically, where the child anchors
// to the parent for the current constraint pair. The drawing is a
// rough mnemonic — the actual layout math lives in
// `kcreate_layout::constraints::apply_constraints` — but it makes
// the difference between e.g. "Min × Max" and "Stretch × Stretch"
// immediately legible without referring to the bridge tests.
function ConstraintVisualiser({
  constraints,
}: {
  constraints: Constraints;
}): JSX.Element {
  const w = 160;
  const h = 90;
  const parent = { x: 8, y: 8, width: w - 16, height: h - 16 };

  // Anchor points on the child rectangle that the constraint pulls
  // toward. (Decorative — not a fidelity match for `apply_constraints`.)
  const child = childRectForConstraints(parent, constraints);
  const leadH = constraints.horizontal === "min" ||
    constraints.horizontal === "stretch";
  const trailH = constraints.horizontal === "max" ||
    constraints.horizontal === "stretch";
  const leadV = constraints.vertical === "min" ||
    constraints.vertical === "stretch";
  const trailV = constraints.vertical === "max" ||
    constraints.vertical === "stretch";

  return (
    <svg
      width={w}
      height={h}
      viewBox={`0 0 ${w} ${h}`}
      style={{
        background: colors.bgSoft,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.md,
      }}
      role="img"
      aria-label={`Constraints preview: ${constraints.horizontal} horizontal, ${constraints.vertical} vertical`}
    >
      <rect
        x={parent.x}
        y={parent.y}
        width={parent.width}
        height={parent.height}
        fill="none"
        stroke={colors.border}
        strokeDasharray="2 3"
        strokeWidth={1}
      />
      <rect
        x={child.x}
        y={child.y}
        width={child.width}
        height={child.height}
        fill={colors.accentBgSoft}
        stroke={colors.accent}
        strokeWidth={1.5}
        rx={3}
      />
      {leadH ? (
        <line
          x1={parent.x}
          y1={child.y + child.height / 2}
          x2={child.x}
          y2={child.y + child.height / 2}
          stroke={colors.accent}
          strokeWidth={1}
        />
      ) : null}
      {trailH ? (
        <line
          x1={child.x + child.width}
          y1={child.y + child.height / 2}
          x2={parent.x + parent.width}
          y2={child.y + child.height / 2}
          stroke={colors.accent}
          strokeWidth={1}
        />
      ) : null}
      {leadV ? (
        <line
          x1={child.x + child.width / 2}
          y1={parent.y}
          x2={child.x + child.width / 2}
          y2={child.y}
          stroke={colors.accent}
          strokeWidth={1}
        />
      ) : null}
      {trailV ? (
        <line
          x1={child.x + child.width / 2}
          y1={child.y + child.height}
          x2={child.x + child.width / 2}
          y2={parent.y + parent.height}
          stroke={colors.accent}
          strokeWidth={1}
        />
      ) : null}
    </svg>
  );
}

interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

function childRectForConstraints(parent: Rect, c: Constraints): Rect {
  const margin = 16;
  let x: number;
  let y: number;
  let width: number;
  let height: number;
  switch (c.horizontal) {
    case "fixed":
    case "min":
    case "scale":
      x = parent.x + margin;
      width = parent.width / 2;
      break;
    case "max":
      x = parent.x + parent.width - margin - parent.width / 2;
      width = parent.width / 2;
      break;
    case "center":
      x = parent.x + parent.width / 2 - parent.width / 4;
      width = parent.width / 2;
      break;
    case "stretch":
      x = parent.x + margin / 2;
      width = parent.width - margin;
      break;
    default: {
      const never: never = c.horizontal;
      throw new Error(`unhandled horizontal constraint: ${String(never)}`);
    }
  }
  switch (c.vertical) {
    case "fixed":
    case "min":
    case "scale":
      y = parent.y + margin / 2;
      height = parent.height / 2;
      break;
    case "max":
      y = parent.y + parent.height - margin / 2 - parent.height / 2;
      height = parent.height / 2;
      break;
    case "center":
      y = parent.y + parent.height / 2 - parent.height / 4;
      height = parent.height / 2;
      break;
    case "stretch":
      y = parent.y + margin / 4;
      height = parent.height - margin / 2;
      break;
    default: {
      const never: never = c.vertical;
      throw new Error(`unhandled vertical constraint: ${String(never)}`);
    }
  }
  return { x, y, width, height };
}

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

const selectStyle: React.CSSProperties = {
  width: "100%",
  padding: 4,
  fontSize: 12,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.sm,
  background: colors.bg,
  color: colors.text,
  boxSizing: "border-box",
};

const fieldLabelStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 2,
  fontSize: 11,
  fontWeight: 600,
  color: colors.textMuted,
};

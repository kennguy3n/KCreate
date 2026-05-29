// Phase 9 Block D Task 23 — Multi-select alignment + distribution.
//
// Wraps `window.kcreate.phase9.documentAlign` and `documentDistribute`
// into a row of buttons. The host passes the current multi-selection
// (1+ node IDs); the toolbar is disabled when fewer than 2 nodes are
// selected since align/distribute only make sense across a group.

import { useCallback, useState } from "react";
import type { Alignment, DistributeAxis } from "../../../shared/scene";
import { colors, font, radius, spacing } from "../styles/tokens";

interface AlignmentToolbarProps {
  selectedNodeIds: string[];
  onApplied?: () => void;
}

const ALIGNMENTS: ReadonlyArray<{ value: Alignment; label: string }> = [
  { value: "left", label: "Align left" },
  { value: "center", label: "Align center (X)" },
  { value: "right", label: "Align right" },
  { value: "top", label: "Align top" },
  { value: "middle", label: "Align middle (Y)" },
  { value: "bottom", label: "Align bottom" },
];

const DISTRIBUTIONS: ReadonlyArray<{
  axis: DistributeAxis;
  label: string;
}> = [
  { axis: "horizontal", label: "Distribute horizontal" },
  { axis: "vertical", label: "Distribute vertical" },
];

export function AlignmentToolbar({
  selectedNodeIds,
  onApplied,
}: AlignmentToolbarProps): JSX.Element {
  const [error, setError] = useState<string | undefined>(undefined);
  const enabled = selectedNodeIds.length >= 2;

  const align = useCallback(
    async (a: Alignment) => {
      setError(undefined);
      try {
        await window.kcreate.phase9.documentAlign(selectedNodeIds, a);
        onApplied?.();
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    },
    [selectedNodeIds, onApplied],
  );

  const distribute = useCallback(
    async (axis: DistributeAxis) => {
      setError(undefined);
      try {
        await window.kcreate.phase9.documentDistribute(selectedNodeIds, axis);
        onApplied?.();
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    },
    [selectedNodeIds, onApplied],
  );

  return (
    <div style={containerStyle} data-testid="kcreate-alignment-toolbar">
      <div style={rowStyle}>
        {ALIGNMENTS.map((a) => (
          <button
            key={a.value}
            type="button"
            onClick={() => void align(a.value)}
            disabled={!enabled}
            title={a.label}
            aria-label={a.label}
            data-testid={`kcreate-align-${a.value}`}
            style={buttonStyle(enabled)}
          >
            {labelFor(a.value)}
          </button>
        ))}
        {DISTRIBUTIONS.map((d) => (
          <button
            key={d.axis}
            type="button"
            onClick={() => void distribute(d.axis)}
            disabled={selectedNodeIds.length < 3}
            title={d.label}
            aria-label={d.label}
            data-testid={`kcreate-distribute-${d.axis}`}
            style={buttonStyle(selectedNodeIds.length >= 3)}
          >
            {d.axis === "horizontal" ? "↔" : "↕"}
          </button>
        ))}
      </div>
      {error !== undefined && (
        <p role="alert" style={errorStyle}>
          {error}
        </p>
      )}
      {!enabled && (
        <p style={hintStyle}>Select 2+ nodes to align, 3+ to distribute.</p>
      )}
    </div>
  );
}

function labelFor(a: Alignment): string {
  switch (a) {
    case "left":
      return "⇤";
    case "center":
      return "⇔";
    case "right":
      return "⇥";
    case "top":
      return "⇡";
    case "middle":
      return "⇕";
    case "bottom":
      return "⇣";
  }
}

const containerStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.xs,
  padding: spacing.sm,
  fontFamily: font.family,
  fontSize: 12,
  color: colors.text,
};

const rowStyle: React.CSSProperties = {
  display: "flex",
  gap: 4,
  flexWrap: "wrap",
};

function buttonStyle(enabled: boolean): React.CSSProperties {
  return {
    background: enabled ? colors.bg : colors.bgSoft,
    color: enabled ? colors.text : colors.textMuted,
    border: `1px solid ${colors.border}`,
    borderRadius: radius.sm,
    padding: "4px 8px",
    fontSize: 14,
    cursor: enabled ? "pointer" : "not-allowed",
    fontWeight: 600,
  };
}

const errorStyle: React.CSSProperties = {
  margin: 0,
  color: "#B91C1C",
  fontSize: 12,
};

const hintStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 11,
  color: colors.textMuted,
  fontStyle: "italic",
};

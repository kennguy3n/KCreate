// PrototypePlayer — Phase 1, Block A, Task 2.
//
// Full-screen overlay that renders the active artboard as the
// "current frame" and lets the user click through hotspots wired up
// via `Interaction.action.NavigateTo` / `Back` / `OpenOverlay` /
// `CloseOverlay`. The player drives off the bridge's
// `window.kcreate.interaction.list()` for every visible node + the
// `window.kcreate.artboard.list()` for the artboard catalog — no
// duplicated state, no mock data.
//
// The "frame" is the current Rust-rendered canvas; we don't re-render
// here, we let the existing CanvasHost present the artboard and we
// overlay invisible hotspot rectangles on top. The player closes when
// the user hits Escape, hits the "Exit" button, or the host clears
// the prop.

import { useCallback, useEffect, useMemo, useState } from "react";

import type { ArtboardInfo, Interaction, NodeInfo } from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface PrototypePlayerProps {
  open: boolean;
  /** Project tree (so we can resolve hotspots on the active artboard). */
  tree: NodeInfo[];
  artboards: ArtboardInfo[];
  /** Initial artboard. If null, defaults to the first artboard. */
  startArtboardId?: string | null;
  onClose: () => void;
}

interface Hotspot {
  nodeId: string;
  interactionId: string;
  /** Bounds in artboard-local coordinates (px). */
  bounds: { x: number; y: number; width: number; height: number };
  action: Interaction["action"];
  trigger: Interaction["trigger"];
}

export function PrototypePlayer({
  open,
  tree,
  artboards,
  startArtboardId,
  onClose,
}: PrototypePlayerProps): JSX.Element | null {
  const [current, setCurrent] = useState<string | null>(null);
  const [stack, setStack] = useState<string[]>([]);
  const [hotspots, setHotspots] = useState<Hotspot[]>([]);
  const [loading, setLoading] = useState<boolean>(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Reset on open with the requested or fallback artboard.
  useEffect(() => {
    if (!open) return;
    const initial = startArtboardId ?? artboards[0]?.id ?? null;
    setCurrent(initial);
    setStack([]);
    setErrorMsg(initial ? null : "No artboards in project.");
  }, [open, startArtboardId, artboards]);

  const currentArtboard = useMemo<ArtboardInfo | null>(
    () => artboards.find((a) => a.id === current) ?? null,
    [artboards, current],
  );

  // Resolve hotspots for the current artboard.
  const refreshHotspots = useCallback(async (): Promise<void> => {
    if (!currentArtboard) {
      setHotspots([]);
      return;
    }
    setLoading(true);
    setErrorMsg(null);
    try {
      const subtree = collectSubtree(tree, currentArtboard.id);
      const collected: Hotspot[] = [];
      for (const node of subtree) {
        const interactions = await window.kcreate.interaction.list(node.id);
        for (const it of interactions) {
          const bounds = readBoundsFromMetadata(node);
          if (!bounds) continue;
          collected.push({
            nodeId: node.id,
            interactionId: it.id,
            bounds,
            action: it.action,
            trigger: it.trigger,
          });
        }
      }
      setHotspots(collected);
    } catch (e) {
      setErrorMsg(errorMessage(e));
    } finally {
      setLoading(false);
    }
  }, [currentArtboard, tree]);

  useEffect(() => {
    void refreshHotspots();
  }, [refreshHotspots]);

  // Keyboard handling: Escape closes, Backspace navigates back.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") {
        e.preventDefault();
        onClose();
        return;
      }
      if (e.key === "Backspace") {
        e.preventDefault();
        navigateBack();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
    };
    // navigateBack is intentionally not a dep: it reads from state
    // closures, and re-binding on every push would only matter if we
    // were synchronously dispatching, which we aren't.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, onClose, stack.length]);

  const navigateTo = (artboardId: string): void => {
    if (current !== null) setStack((prev) => [...prev, current]);
    setCurrent(artboardId);
  };

  const navigateBack = (): void => {
    setStack((prev) => {
      if (prev.length === 0) return prev;
      const next = prev.slice(0, -1);
      const last = prev[prev.length - 1];
      if (last !== undefined) setCurrent(last);
      return next;
    });
  };

  const fireHotspot = (h: Hotspot): void => {
    switch (h.action.kind) {
      case "navigate_to":
        navigateTo(h.action.target_artboard_id);
        break;
      case "open_overlay":
        navigateTo(h.action.overlay_artboard_id);
        break;
      case "close_overlay":
      case "back":
        navigateBack();
        break;
      case "scroll_to":
        // Visual scroll is owned by the canvas host; we surface a
        // status hint only.
        break;
    }
  };

  if (!open) return null;

  return (
    <div style={overlayStyle} role="dialog" aria-label="Prototype player">
      <header style={headerStyle}>
        <div style={crumbStyle} aria-label="Navigation history">
          {[...stack, current].filter(Boolean).map((id, idx, arr) => {
            const ab = artboards.find((a) => a.id === id);
            const label = ab?.name ?? "?";
            const isLast = idx === arr.length - 1;
            return (
              <span key={`${id ?? "x"}-${idx}`} style={crumbItemStyle(isLast)}>
                {label}
                {idx < arr.length - 1 ? " ›" : null}
              </span>
            );
          })}
        </div>
        <div style={controlsRowStyle}>
          {stack.length > 0 ? (
            <button
              type="button"
              onClick={navigateBack}
              style={controlBtn()}
              aria-label="Go back"
            >
              ← Back
            </button>
          ) : null}
          <button
            type="button"
            onClick={onClose}
            style={controlBtn(true)}
            aria-label="Exit prototype"
          >
            Exit
          </button>
        </div>
      </header>
      <div style={frameWrapperStyle}>
        {currentArtboard ? (
          <div
            style={frameInnerStyle(currentArtboard.width, currentArtboard.height)}
          >
            <div style={frameLabelStyle}>
              {currentArtboard.name} · {Math.round(currentArtboard.width)}×
              {Math.round(currentArtboard.height)}
            </div>
            {hotspots.map((h) => (
              <button
                key={h.interactionId}
                type="button"
                onClick={() => fireHotspot(h)}
                style={hotspotStyle(h.bounds)}
                title={`${h.trigger} → ${describeAction(h.action, artboards)}`}
                aria-label={`Interaction hotspot on ${h.nodeId}`}
              />
            ))}
            {loading ? (
              <div style={loadingPillStyle}>Loading hotspots…</div>
            ) : null}
            {errorMsg ? (
              <div style={errorPillStyle}>{errorMsg}</div>
            ) : null}
          </div>
        ) : (
          <div style={emptyStateStyle}>
            {errorMsg ??
              "Add an artboard with interactions to start a prototype flow."}
          </div>
        )}
      </div>
    </div>
  );
}

function collectSubtree(tree: NodeInfo[], rootId: string): NodeInfo[] {
  const byId = new Map<string, NodeInfo>();
  for (const n of tree) byId.set(n.id, n);
  const out: NodeInfo[] = [];
  const stack: string[] = [rootId];
  while (stack.length > 0) {
    const id = stack.pop();
    if (id === undefined) continue;
    const node = byId.get(id);
    if (!node) continue;
    out.push(node);
    for (const c of node.children) stack.push(c);
  }
  return out;
}

function readBoundsFromMetadata(node: NodeInfo): {
  x: number;
  y: number;
  width: number;
  height: number;
} | null {
  const m = node.metadata;
  if (!m || typeof m !== "object") return null;
  const b = (m as { bounds?: unknown }).bounds;
  if (!b || typeof b !== "object") return null;
  const r = b as Record<string, unknown>;
  if (
    typeof r.x === "number" &&
    typeof r.y === "number" &&
    typeof r.width === "number" &&
    typeof r.height === "number"
  ) {
    return { x: r.x, y: r.y, width: r.width, height: r.height };
  }
  return null;
}

function describeAction(
  action: Interaction["action"],
  artboards: ArtboardInfo[],
): string {
  const lookup = (id: string): string =>
    artboards.find((a) => a.id === id)?.name ?? `${id.slice(0, 8)}…`;
  switch (action.kind) {
    case "navigate_to":
      return `Navigate to ${lookup(action.target_artboard_id)}`;
    case "open_overlay":
      return `Open overlay ${lookup(action.overlay_artboard_id)}`;
    case "close_overlay":
      return "Close overlay";
    case "back":
      return "Back";
    case "scroll_to":
      return `Scroll to ${action.target_node_id.slice(0, 8)}…`;
  }
}

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

const overlayStyle: React.CSSProperties = {
  position: "fixed",
  inset: 0,
  background: "rgba(17, 24, 39, 0.94)",
  display: "flex",
  flexDirection: "column",
  zIndex: 1000,
};

const headerStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  padding: `${spacing.sm}px ${spacing.md}px`,
  background: "rgba(0,0,0,0.4)",
  color: colors.textInverse,
  fontSize: 12,
};

const crumbStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: spacing.xs,
  flexWrap: "wrap",
};

function crumbItemStyle(active: boolean): React.CSSProperties {
  return {
    color: active ? colors.textInverse : "rgba(255,255,255,0.6)",
    fontWeight: active ? 600 : 400,
  };
}

const controlsRowStyle: React.CSSProperties = {
  display: "flex",
  gap: spacing.xs,
};

function controlBtn(primary = false): React.CSSProperties {
  return {
    padding: "6px 12px",
    fontSize: 11,
    fontWeight: 600,
    background: primary ? colors.accent : "transparent",
    color: colors.textInverse,
    border: `1px solid ${primary ? colors.accent : "rgba(255,255,255,0.5)"}`,
    borderRadius: radius.pill,
    cursor: "pointer",
  };
}

const frameWrapperStyle: React.CSSProperties = {
  flex: 1,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  padding: spacing.md,
  overflow: "auto",
};

function frameInnerStyle(width: number, height: number): React.CSSProperties {
  return {
    position: "relative",
    width,
    height,
    background: colors.bg,
    borderRadius: radius.card,
    boxShadow: "0 12px 48px rgba(0,0,0,0.4)",
    overflow: "hidden",
  };
}

const frameLabelStyle: React.CSSProperties = {
  position: "absolute",
  top: 8,
  left: 8,
  background: "rgba(17,24,39,0.65)",
  color: colors.textInverse,
  padding: "4px 10px",
  borderRadius: radius.pill,
  fontSize: 10,
  fontWeight: 500,
  pointerEvents: "none",
};

function hotspotStyle(bounds: {
  x: number;
  y: number;
  width: number;
  height: number;
}): React.CSSProperties {
  return {
    position: "absolute",
    left: bounds.x,
    top: bounds.y,
    width: bounds.width,
    height: bounds.height,
    background: "rgba(124, 58, 237, 0.0)",
    border: "1px dashed rgba(124, 58, 237, 0.0)",
    cursor: "pointer",
    padding: 0,
    transition: "background 120ms ease, border-color 120ms ease",
  };
}

const loadingPillStyle: React.CSSProperties = {
  position: "absolute",
  bottom: 8,
  right: 8,
  background: "rgba(17,24,39,0.65)",
  color: colors.textInverse,
  padding: "4px 10px",
  borderRadius: radius.pill,
  fontSize: 10,
};

const errorPillStyle: React.CSSProperties = {
  position: "absolute",
  bottom: 8,
  right: 8,
  background: "rgba(220,38,38,0.85)",
  color: colors.textInverse,
  padding: "4px 10px",
  borderRadius: radius.pill,
  fontSize: 10,
};

const emptyStateStyle: React.CSSProperties = {
  padding: spacing.lg,
  background: colors.bg,
  borderRadius: radius.card,
  color: colors.textMuted,
};

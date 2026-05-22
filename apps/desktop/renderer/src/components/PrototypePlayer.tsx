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

  // Reset on open with the requested or fallback artboard. Depend on
  // a stable string fingerprint of the artboards (first-id + length)
  // instead of the array reference so the player doesn't reset itself
  // mid-session when the parent rebuilds its artboards array but the
  // identities are unchanged. We only need the first artboard for the
  // fallback path, so keying off its id + the count is sufficient to
  // detect "the catalog actually changed" without tracking every entry.
  const firstArtboardId = artboards[0]?.id ?? null;
  const artboardCount = artboards.length;
  useEffect(() => {
    if (!open) return;
    const initial = startArtboardId ?? firstArtboardId ?? null;
    setCurrent(initial);
    setStack([]);
    setErrorMsg(initial ? null : "No artboards in project.");
  }, [open, startArtboardId, firstArtboardId, artboardCount]);

  const currentArtboard = useMemo<ArtboardInfo | null>(
    () => artboards.find((a) => a.id === current) ?? null,
    [artboards, current],
  );

  // Resolve hotspots for the current artboard.
  //
  // Devin Review ANALYSIS-0003: this used to fire one IPC round trip
  // per node in the artboard subtree, which for a heavy artboard
  // (large card grids, deep groups) added perceptible lag every time
  // the user navigated to a new screen. We now request the whole
  // subtree's interactions in a single `interaction.listBatch` call.
  // Nodes with no interactions are omitted from the batch result, so
  // a missing key in `byNode` is equivalent to an empty list.
  const refreshHotspots = useCallback(async (): Promise<void> => {
    if (!currentArtboard) {
      setHotspots([]);
      return;
    }
    setLoading(true);
    setErrorMsg(null);
    try {
      const subtree = collectSubtree(tree, currentArtboard.id);
      const ids = subtree.map((n) => n.id);
      const byNode = await window.kcreate.interaction.listBatch(ids);
      const collected: Hotspot[] = [];
      for (const node of subtree) {
        const interactions = byNode[node.id];
        if (!interactions || interactions.length === 0) continue;
        // `bounds` is now first-class on the wire shape — see the
        // BoundsInfo addition in `kcreate_bridge::document::NodeInfo`.
        // A zero-size box is still a valid bound (e.g. a freshly
        // created group before layout), but it would never receive a
        // click, so we drop it from the hotspot set to avoid
        // rendering invisible overlays the user can't hit.
        const b = node.bounds;
        if (b.width <= 0 || b.height <= 0) continue;
        for (const it of interactions) {
          collected.push({
            nodeId: node.id,
            interactionId: it.id,
            bounds: {
              x: b.x - currentArtboard.x,
              y: b.y - currentArtboard.y,
              width: b.width,
              height: b.height,
            },
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
    // `navigateBack` reads from state via the functional `setStack`
    // updater, so it doesn't need to be in the deps list — re-binding
    // when only `stack.length` changes is sufficient to keep the key
    // handler reading a current closure for the few non-state values
    // it actually closes over.
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

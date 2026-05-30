// PrototypePlayer — Phase 1 Block A Task 2; substantially extended in
// Phase 11 Block C Tasks 14 / 15 / 17.
//
// Full-screen overlay that renders the active artboard as the
// "current frame" and lets the user click through hotspots wired up
// via the bridge's `Interaction.action` family. The player drives
// off `window.kcreate.interaction.listBatch()` and
// `window.kcreate.artboard.list()` — no mock data, no duplicated
// state.
//
// Phase 11 capabilities layered on top of the original:
//
//  * Pluggable trigger model: hotspots respond to Click / Hover /
//    Press (with a pressed visual state) / MouseEnter / MouseLeave,
//    and the artboard itself fires `AfterDelay` triggers attached
//    to its descendants.
//  * Transition animation engine: NavigateTo / OpenOverlay /
//    SwitchVariant interactions carry a `Transition` config
//    (Instant / Dissolve / SlideIn / SlideOut / Push / MoveIn)
//    driven by `requestAnimationFrame` and the easing curves in
//    `../lib/EasingEngine`.
//  * SwitchVariant (Smart Animate): when an interaction fires a
//    variant switch, we crossfade the source artboard out and the
//    target variant artboard in over the configured duration.
//
// All renderer changes are presentation-only — Rust still owns the
// scene graph, the Operation log, and the document model.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type {
  AnimationType,
  ArtboardInfo,
  Interaction,
  NodeInfo,
  SlideDirection,
  SmartAnimateLayer,
  SmartAnimateSnapshot,
  Transition,
} from "../../../shared/scene";
import { sample as sampleEasing } from "../lib/EasingEngine";
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

/** Live transition state driven by `requestAnimationFrame`. */
interface ActiveTransition {
  fromArtboardId: string;
  toArtboardId: string;
  config: Transition;
  /** Wall-clock millis when the animation started. */
  startedAt: number;
  /** Normalised progress, 0 → 1. Updated each rAF tick. */
  progress: number;
}

/**
 * Phase 11 Block C Task 17 — live Smart Animate state for a
 * variant switch. The player paints an overlay of the before/after
 * layer rectangles interpolated by name match, then commits the
 * variant switch via the bridge once `progress === 1`.
 */
interface ActiveSmartAnimate {
  instanceNodeId: string;
  variantId: string;
  snapshot: SmartAnimateSnapshot;
  config: Transition;
  startedAt: number;
  progress: number;
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
  const stackRef = useRef<string[]>([]);
  useEffect(() => {
    stackRef.current = stack;
  }, [stack]);
  const [hotspots, setHotspots] = useState<Hotspot[]>([]);
  const [loading, setLoading] = useState<boolean>(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [pressedHotspot, setPressedHotspot] = useState<string | null>(null);
  const [activeTransition, setActiveTransition] =
    useState<ActiveTransition | null>(null);
  const [activeSmartAnimate, setActiveSmartAnimate] =
    useState<ActiveSmartAnimate | null>(null);

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
  }, [open, onClose, stack.length]);

  // `AfterDelay` triggers — fire when the player has been on the
  // current artboard for the configured number of milliseconds.
  // Cleared when the player navigates away (the cleanup function
  // runs because `current` changes), the player closes, or the
  // hotspot list refreshes.
  useEffect(() => {
    if (!open || !current) return;
    const timeouts: number[] = [];
    for (const h of hotspots) {
      if (typeof h.trigger === "object" && h.trigger.kind === "after_delay") {
        const id = window.setTimeout(
          () => fireHotspotRef.current?.(h),
          Math.max(0, h.trigger.ms),
        );
        timeouts.push(id);
      }
    }
    return () => {
      for (const id of timeouts) window.clearTimeout(id);
    };
  }, [open, current, hotspots]);

  // Animation loop driven by requestAnimationFrame.
  useEffect(() => {
    if (!activeTransition) return;
    let raf = 0;
    const tick = (): void => {
      const now = performance.now();
      const elapsed = now - activeTransition.startedAt;
      const dur = Math.max(1, activeTransition.config.duration_ms);
      const t = Math.min(1, elapsed / dur);
      setActiveTransition((prev) =>
        prev === null ? null : { ...prev, progress: t },
      );
      if (t < 1) {
        raf = window.requestAnimationFrame(tick);
      } else {
        setCurrent(activeTransition.toArtboardId);
        setActiveTransition(null);
      }
    };
    raf = window.requestAnimationFrame(tick);
    return () => {
      window.cancelAnimationFrame(raf);
    };
    // `setActiveTransition` updates within the loop set the `.progress`
    // field but don't replace the transition reference; we want to keep
    // the rAF loop running for the lifetime of this transition.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeTransition?.fromArtboardId, activeTransition?.toArtboardId]);

  // Phase 11 Block C Task 17 — Smart Animate rAF loop. Drives the
  // property-interpolation overlay forward and commits the variant
  // switch via the bridge once progress reaches 1.0.
  useEffect(() => {
    if (!activeSmartAnimate) return;
    let raf = 0;
    let committed = false;
    const tick = (): void => {
      const now = performance.now();
      const elapsed = now - activeSmartAnimate.startedAt;
      const dur = Math.max(1, activeSmartAnimate.config.duration_ms);
      const t = Math.min(1, elapsed / dur);
      setActiveSmartAnimate((prev) =>
        prev === null ? null : { ...prev, progress: t },
      );
      if (t < 1) {
        raf = window.requestAnimationFrame(tick);
        return;
      }
      if (!committed) {
        committed = true;
        // Commit the variant swap on the document graph. The
        // player keeps showing the final interpolated frame until
        // the next scene-sync repaint catches up.
        void window.kcreate.component
          .switchVariant(
            activeSmartAnimate.instanceNodeId,
            activeSmartAnimate.variantId,
          )
          .catch((e: unknown) => {
            setErrorMsg(errorMessage(e));
          })
          .finally(() => {
            setActiveSmartAnimate(null);
          });
      }
    };
    raf = window.requestAnimationFrame(tick);
    return () => {
      window.cancelAnimationFrame(raf);
    };
    // The rAF loop tracks the lifetime of this Smart Animate
    // snapshot; we re-run the effect only when a new snapshot
    // begins, not on every `progress` setState.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeSmartAnimate?.instanceNodeId, activeSmartAnimate?.variantId]);

  const navigateTo = (
    artboardId: string,
    transition: Transition | undefined,
  ): void => {
    if (current === null) {
      setCurrent(artboardId);
      return;
    }
    setStack((prev) => [...prev, current]);
    if (
      !transition ||
      transition.animation === "instant" ||
      transition.duration_ms <= 0
    ) {
      setCurrent(artboardId);
      return;
    }
    setActiveTransition({
      fromArtboardId: current,
      toArtboardId: artboardId,
      config: transition,
      startedAt: performance.now(),
      progress: 0,
    });
  };

  const navigateBack = (): void => {
    const snapshot = stackRef.current;
    if (snapshot.length === 0) return;
    const last = snapshot[snapshot.length - 1];
    if (last !== undefined) setCurrent(last);
    setStack((prev) => (prev.length === 0 ? prev : prev.slice(0, -1)));
  };

  // Note: not wrapped in `useCallback` — closures captured here
  // reference `current`, `navigateTo`, and `navigateBack`, all of
  // which change identity every render. We bridge stale references
  // via `fireHotspotRef` below so consumers (timeouts, child
  // components) always see the freshest closure.
  const fireHotspot = (h: Hotspot): void => {
    switch (h.action.kind) {
      case "navigate_to":
        navigateTo(h.action.target_artboard_id, h.action.transition);
        break;
      case "open_overlay":
        navigateTo(h.action.overlay_artboard_id, h.action.transition);
        break;
      case "close_overlay":
      case "back":
        navigateBack();
        break;
      case "scroll_to":
        // Visual scroll is owned by the canvas host; we surface a
        // status hint only.
        break;
      case "switch_variant": {
        // Phase 11 Block C Task 17 — Smart Animate. Fetch the
        // before/after layer snapshot from the bridge, then play
        // a property-interpolation overlay over the transition's
        // duration; commit the variant swap once the animation
        // finishes. The instance node is the same node carrying
        // the interaction (hotspots are attached to
        // `ComponentLayer` instances in the editor).
        const action = h.action;
        const instanceId = h.nodeId;
        const variantId = action.variant_id;
        // `transition` is optional on the wire — fall back to the
        // same default the bridge stamps on legacy interactions
        // (Instant / 300ms / ease-in-out) so older projects open
        // without surprises.
        const transition: Transition = action.transition ?? {
          animation: "instant",
          duration_ms: 300,
          easing: { kind: "ease_in_out" },
          direction: null,
        };
        // Phase 11 Block C follow-up round 5 — Devin Review
        // BUG-0001 (r5). Mirror the instant-transition guard used
        // by navigate_to / open_overlay (above): when the
        // transition is Instant or its duration is non-positive,
        // commit the variant swap directly via `switchVariant`
        // instead of running the Smart Animate overlay. The
        // InteractionPanel hides duration controls for "instant",
        // so its `durationMs` state may still hold a stale 300ms
        // value — running the overlay would visually animate for
        // 300ms even though the user explicitly chose Instant.
        if (
          transition.animation === "instant" ||
          transition.duration_ms <= 0
        ) {
          void window.kcreate.component
            .switchVariant(instanceId, variantId)
            .catch((e: unknown) => {
              setErrorMsg(errorMessage(e));
            });
          break;
        }
        void window.kcreate.component
          .smartAnimateSnapshot(instanceId, variantId)
          .then((snapshot) => {
            setActiveSmartAnimate({
              instanceNodeId: instanceId,
              variantId,
              snapshot,
              config: transition,
              startedAt: performance.now(),
              progress: 0,
            });
          })
          .catch((e: unknown) => {
            setErrorMsg(errorMessage(e));
          });
        break;
      }
    }
  };

  // Ref kept in sync with the latest `fireHotspot` closure so the
  // `AfterDelay` timeouts (which capture the function once) always
  // see the freshest closure. We deliberately update the ref on
  // every render rather than via a separate `useEffect` — see
  // https://react.dev/reference/react/useRef#avoiding-recreating-the-ref-contents
  const fireHotspotRef = useRef<((h: Hotspot) => void) | null>(null);
  fireHotspotRef.current = fireHotspot;

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
          <PrototypeFrame
            artboard={currentArtboard}
            hotspots={hotspots}
            pressedHotspot={pressedHotspot}
            setPressedHotspot={setPressedHotspot}
            fireHotspot={fireHotspot}
            transition={activeTransition}
            smartAnimate={activeSmartAnimate}
            outgoingArtboard={
              activeTransition
                ? artboards.find((a) => a.id === activeTransition.fromArtboardId) ?? null
                : null
            }
            incomingArtboard={
              activeTransition
                ? artboards.find((a) => a.id === activeTransition.toArtboardId) ?? null
                : null
            }
            loading={loading}
            errorMsg={errorMsg}
          />
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

interface PrototypeFrameProps {
  artboard: ArtboardInfo;
  hotspots: Hotspot[];
  pressedHotspot: string | null;
  setPressedHotspot: (id: string | null) => void;
  fireHotspot: (h: Hotspot) => void;
  transition: ActiveTransition | null;
  smartAnimate: ActiveSmartAnimate | null;
  outgoingArtboard: ArtboardInfo | null;
  incomingArtboard: ArtboardInfo | null;
  loading: boolean;
  errorMsg: string | null;
}

function PrototypeFrame({
  artboard,
  hotspots,
  pressedHotspot,
  setPressedHotspot,
  fireHotspot,
  transition,
  smartAnimate,
  outgoingArtboard,
  incomingArtboard,
  loading,
  errorMsg,
}: PrototypeFrameProps): JSX.Element {
  // When an animation is in flight, we render two layered frames:
  // the *outgoing* (current) artboard and the *incoming* (target).
  // Hotspots only attach to the outgoing layer to avoid double
  // event handling. Once the transition finishes, the controlling
  // `PrototypePlayer` swaps `current` to the target artboard,
  // dropping the overlay back to a single layer.
  if (transition && outgoingArtboard && incomingArtboard) {
    const p = sampleEasing(transition.config.easing, transition.progress);
    const layers = animationLayers(transition.config.animation, transition.config.direction ?? null, p);
    return (
      <div style={frameInnerStyle(artboard.width, artboard.height)}>
        <AnimatedLayer
          artboard={outgoingArtboard}
          style={layers.outgoing}
          isInteractive={false}
          hotspots={[]}
          pressedHotspot={null}
          setPressedHotspot={() => {}}
          fireHotspot={() => {}}
        />
        <AnimatedLayer
          artboard={incomingArtboard}
          style={layers.incoming}
          isInteractive={false}
          hotspots={[]}
          pressedHotspot={null}
          setPressedHotspot={() => {}}
          fireHotspot={() => {}}
        />
        <div style={loadingPillStyle}>
          Animating ({Math.round(transition.progress * 100)}%)
        </div>
      </div>
    );
  }

  return (
    <div style={frameInnerStyle(artboard.width, artboard.height)}>
      <div style={frameLabelStyle}>
        {artboard.name} · {Math.round(artboard.width)}×
        {Math.round(artboard.height)}
      </div>
      {hotspots.map((h) => (
        <HotspotButton
          key={h.interactionId}
          hotspot={h}
          pressed={pressedHotspot === h.interactionId}
          setPressed={setPressedHotspot}
          onFire={fireHotspot}
        />
      ))}
      {smartAnimate ? (
        <SmartAnimateOverlay
          artboard={artboard}
          state={smartAnimate}
        />
      ) : null}
      {loading ? <div style={loadingPillStyle}>Loading hotspots…</div> : null}
      {errorMsg ? <div style={errorPillStyle}>{errorMsg}</div> : null}
    </div>
  );
}

/**
 * Phase 11 Block C Task 17 — overlay that paints the interpolated
 * layer rectangles between the before / after Smart Animate
 * snapshots. Layers in `before` but not in `after` (matched by
 * `name`) fade out; layers in `after` but not in `before` fade in.
 */
interface SmartAnimateOverlayProps {
  artboard: ArtboardInfo;
  state: ActiveSmartAnimate;
}

function SmartAnimateOverlay({
  artboard,
  state,
}: SmartAnimateOverlayProps): JSX.Element {
  const eased = sampleEasing(state.config.easing, state.progress);
  const beforeByName = new Map<string, SmartAnimateLayer>();
  for (const b of state.snapshot.before) {
    beforeByName.set(b.name, b);
  }
  const afterByName = new Map<string, SmartAnimateLayer>();
  for (const a of state.snapshot.after) {
    afterByName.set(a.name, a);
  }
  const names = new Set<string>([
    ...beforeByName.keys(),
    ...afterByName.keys(),
  ]);
  const rects: JSX.Element[] = [];
  for (const name of names) {
    const a = beforeByName.get(name);
    const b = afterByName.get(name);
    if (a && b) {
      // Matched pair — interpolate bounds + opacity + colour +
      // corner radius.
      const x = lerp(a.bounds.x, b.bounds.x, eased) - artboard.x;
      const y = lerp(a.bounds.y, b.bounds.y, eased) - artboard.y;
      const w = lerp(a.bounds.width, b.bounds.width, eased);
      const h = lerp(a.bounds.height, b.bounds.height, eased);
      const opacity = lerp(a.opacity, b.opacity, eased);
      const radiusPx = lerp(a.corner_radius, b.corner_radius, eased);
      const bg = lerpHexHsl(a.fill_color, b.fill_color, eased);
      rects.push(
        <div
          key={`m:${name}`}
          aria-hidden="true"
          style={{
            position: "absolute",
            left: x,
            top: y,
            width: w,
            height: h,
            opacity,
            background: bg ?? "transparent",
            borderRadius: radiusPx,
            pointerEvents: "none",
          }}
        />,
      );
    } else if (a && !b) {
      // Fades out — opacity goes from a.opacity → 0.
      const opacity = a.opacity * (1 - eased);
      rects.push(
        <div
          key={`out:${name}`}
          aria-hidden="true"
          style={{
            position: "absolute",
            left: a.bounds.x - artboard.x,
            top: a.bounds.y - artboard.y,
            width: a.bounds.width,
            height: a.bounds.height,
            opacity,
            background: a.fill_color ?? "transparent",
            borderRadius: a.corner_radius,
            pointerEvents: "none",
          }}
        />,
      );
    } else if (!a && b) {
      // Fades in — opacity goes from 0 → b.opacity.
      const opacity = b.opacity * eased;
      rects.push(
        <div
          key={`in:${name}`}
          aria-hidden="true"
          style={{
            position: "absolute",
            left: b.bounds.x - artboard.x,
            top: b.bounds.y - artboard.y,
            width: b.bounds.width,
            height: b.bounds.height,
            opacity,
            background: b.fill_color ?? "transparent",
            borderRadius: b.corner_radius,
            pointerEvents: "none",
          }}
        />,
      );
    }
  }
  return (
    <div
      aria-hidden="true"
      style={{
        position: "absolute",
        inset: 0,
        pointerEvents: "none",
      }}
    >
      {rects}
      <div style={loadingPillStyle}>
        Smart Animate ({Math.round(state.progress * 100)}%)
      </div>
    </div>
  );
}

function lerp(a: number, b: number, t: number): number {
  return a + (b - a) * t;
}

/**
 * Interpolate two `#RRGGBB` strings in HSL space so hue rotates
 * smoothly. Returns a `hsl(...)` CSS string. Either argument may
 * be `null` — in that case we lerp the other end's opacity (so a
 * gradient → solid transition fades the solid in/out cleanly).
 */
function lerpHexHsl(
  a: string | null,
  b: string | null,
  t: number,
): string | null {
  if (a === null && b === null) return null;
  if (a === null && b !== null) {
    const bh = hexToHsl(b);
    if (bh === null) return b;
    return `hsla(${bh.h}, ${bh.s}%, ${bh.l}%, ${t.toFixed(3)})`;
  }
  if (b === null && a !== null) {
    const ah = hexToHsl(a);
    if (ah === null) return a;
    return `hsla(${ah.h}, ${ah.s}%, ${ah.l}%, ${(1 - t).toFixed(3)})`;
  }
  if (!a || !b) return null;
  const ah = hexToHsl(a);
  const bh = hexToHsl(b);
  if (ah === null || bh === null) return a;
  // Shortest-path hue interpolation so red→magenta doesn't
  // detour through cyan.
  let dh = bh.h - ah.h;
  if (dh > 180) dh -= 360;
  else if (dh < -180) dh += 360;
  const h = (ah.h + dh * t + 360) % 360;
  const s = lerp(ah.s, bh.s, t);
  const l = lerp(ah.l, bh.l, t);
  return `hsl(${h.toFixed(2)}, ${s.toFixed(2)}%, ${l.toFixed(2)}%)`;
}

function hexToHsl(
  hex: string,
): { h: number; s: number; l: number } | null {
  if (hex.length !== 7 || hex[0] !== "#") return null;
  const r = parseInt(hex.slice(1, 3), 16) / 255;
  const g = parseInt(hex.slice(3, 5), 16) / 255;
  const b = parseInt(hex.slice(5, 7), 16) / 255;
  if (Number.isNaN(r) || Number.isNaN(g) || Number.isNaN(b)) return null;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  let h = 0;
  let s = 0;
  const d = max - min;
  if (d > 0) {
    s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
    if (max === r) h = ((g - b) / d + (g < b ? 6 : 0)) * 60;
    else if (max === g) h = ((b - r) / d + 2) * 60;
    else h = ((r - g) / d + 4) * 60;
  }
  return { h, s: s * 100, l: l * 100 };
}

interface AnimatedLayerProps {
  artboard: ArtboardInfo;
  style: React.CSSProperties;
  isInteractive: boolean;
  hotspots: Hotspot[];
  pressedHotspot: string | null;
  setPressedHotspot: (id: string | null) => void;
  fireHotspot: (h: Hotspot) => void;
}

function AnimatedLayer({
  artboard,
  style,
  isInteractive,
  hotspots,
  pressedHotspot,
  setPressedHotspot,
  fireHotspot,
}: AnimatedLayerProps): JSX.Element {
  return (
    <div style={{ ...layerBaseStyle, ...style }}>
      <div style={frameLabelStyle}>{artboard.name}</div>
      {isInteractive
        ? hotspots.map((h) => (
            <HotspotButton
              key={h.interactionId}
              hotspot={h}
              pressed={pressedHotspot === h.interactionId}
              setPressed={setPressedHotspot}
              onFire={fireHotspot}
            />
          ))
        : null}
    </div>
  );
}

interface HotspotButtonProps {
  hotspot: Hotspot;
  pressed: boolean;
  setPressed: (id: string | null) => void;
  onFire: (h: Hotspot) => void;
}

function HotspotButton({
  hotspot,
  pressed,
  setPressed,
  onFire,
}: HotspotButtonProps): JSX.Element {
  const t = hotspot.trigger;
  const triggerKind = typeof t === "string" ? t : t.kind;

  const handlers: React.HTMLAttributes<HTMLButtonElement> = {};
  switch (triggerKind) {
    case "click":
      handlers.onClick = () => onFire(hotspot);
      break;
    case "hover":
    case "mouse_enter":
      handlers.onMouseEnter = () => onFire(hotspot);
      break;
    case "mouse_leave":
      handlers.onMouseLeave = () => onFire(hotspot);
      break;
    case "press":
      handlers.onMouseDown = () => {
        setPressed(hotspot.interactionId);
        onFire(hotspot);
      };
      handlers.onMouseUp = () => setPressed(null);
      handlers.onMouseLeave = () => setPressed(null);
      break;
    case "after_delay":
      // Driven by a timeout in the parent — no DOM listener here.
      break;
    default:
      handlers.onClick = () => onFire(hotspot);
  }

  return (
    <button
      type="button"
      {...handlers}
      style={hotspotStyle(hotspot.bounds, pressed)}
      title={triggerKind}
      aria-label={`Interaction hotspot on ${hotspot.nodeId}`}
    />
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

/**
 * Compute the CSS transform / opacity overrides for the outgoing
 * and incoming layers given an animation type, optional direction,
 * and eased progress `p ∈ [0, 1]`. Exported for unit tests.
 */
export function animationLayers(
  animation: AnimationType,
  direction: SlideDirection | null,
  p: number,
): { outgoing: React.CSSProperties; incoming: React.CSSProperties } {
  const clampedP = Math.max(0, Math.min(1, p));
  switch (animation) {
    case "instant":
      return {
        outgoing: { opacity: clampedP < 1 ? 1 : 0 },
        incoming: { opacity: clampedP < 1 ? 0 : 1 },
      };
    case "dissolve":
      return {
        outgoing: { opacity: 1 - clampedP },
        incoming: { opacity: clampedP },
      };
    case "slide_in":
      return {
        outgoing: { opacity: 1 },
        incoming: {
          transform: slideTransform(direction, 1 - clampedP),
          opacity: 1,
        },
      };
    case "slide_out":
      // Phase 11 Block C follow-up round 4 — Devin Review
      // ANALYSIS-0003 (r4). Figma's "Slide out → <direction>"
      // means the outgoing content exits in that direction. The
      // previous code passed `-clampedP`, which sent the outgoing
      // layer the OPPOSITE way (slide_out + left moved the layer
      // to the right). Flipped the sign so direction describes
      // the visible motion of the outgoing layer, matching Figma
      // semantics that designers using KCreate already expect.
      return {
        outgoing: {
          transform: slideTransform(direction, clampedP),
          opacity: 1,
        },
        incoming: { opacity: 1 },
      };
    case "push":
      // Phase 11 Block C follow-up round 4 — Devin Review
      // ANALYSIS-0003 (r4). Figma's "Push → <direction>" slides
      // both layers in <direction>: the outgoing layer exits in
      // <direction>, and the incoming layer enters from the
      // OPPOSITE side (so it appears to push the outgoing one
      // off). Previous code had the outgoing exiting the wrong
      // way AND the incoming entering from the wrong side. Now:
      // outgoing translates 0 → +direction (Figma exit); incoming
      // translates −direction → 0 (Figma entry from opposite).
      return {
        outgoing: {
          transform: slideTransform(direction, clampedP),
          opacity: 1,
        },
        incoming: {
          transform: slideTransform(direction, clampedP - 1),
          opacity: 1,
        },
      };
    case "move_in":
      return {
        outgoing: { opacity: 1 },
        incoming: {
          transform: slideTransform(direction, 1 - clampedP),
          opacity: 1,
        },
      };
    default: {
      const _exhaustive: never = animation;
      void _exhaustive;
      return {
        outgoing: { opacity: 1 - clampedP },
        incoming: { opacity: clampedP },
      };
    }
  }
}

/**
 * Translate a layer along the given direction by a fraction of its
 * own size. `fraction = 1` translates fully off-screen in the
 * direction's positive axis; `fraction = -1` translates off-screen
 * in the opposite direction.
 */
function slideTransform(
  direction: SlideDirection | null,
  fraction: number,
): string {
  const f = `${(fraction * 100).toFixed(3)}%`;
  switch (direction) {
    case "right":
      return `translateX(${f})`;
    case "up":
      return `translateY(${negate(f)})`;
    case "down":
      return `translateY(${f})`;
    case "left":
    case null:
    default:
      return `translateX(${negate(f)})`;
  }
}

function negate(percent: string): string {
  if (percent.startsWith("-")) return percent.slice(1);
  return `-${percent}`;
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

const layerBaseStyle: React.CSSProperties = {
  position: "absolute",
  inset: 0,
  background: colors.bg,
  willChange: "transform, opacity",
};

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

function hotspotStyle(
  bounds: { x: number; y: number; width: number; height: number },
  pressed: boolean,
): React.CSSProperties {
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
    transition: "transform 100ms ease, opacity 100ms ease, " +
      "background 120ms ease, border-color 120ms ease",
    transform: pressed ? "scale(0.97)" : undefined,
    opacity: pressed ? 0.8 : 1,
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
  background: colors.dangerOverlay,
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

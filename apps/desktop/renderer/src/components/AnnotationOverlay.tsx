// AnnotationOverlay — Phase 8 Block A Task 5.
//
// Renders one coloured pin per annotation on the current page at its
// world-space position, projected through the local viewport. Sits on
// top of `CanvasHost`. Pin clicks open a floating thread panel
// (replies + resolve/delete + reply input). Empty-canvas double-click
// while the overlay is active drops a new pin (and opens the editor
// inline).
//
// Data flow:
//   * On mount + page change, fetch the per-page list via
//     `window.kcreate.annotation.list({ pageId, includeResolved,
//     includeUnresolved })`. Filter is configurable in the floating
//     legend (open-only / resolved-only / all).
//   * Subscribe to `window.kcreate.session.onEvent` and re-fetch
//     when the `annotationsApplied` event mentions our page. This is
//     the same pattern the cursor / selection overlays use — the
//     bridge owns the canonical list, the overlay just re-pulls
//     when the storage layer says peers wrote.
//   * Local mutations (create / reply / resolve / delete) go through
//     `window.kcreate.annotation.*` and re-pull on success so the
//     pin set reflects post-mutation state.
//
// Author identity:
//   * `authorPeerId` / `authorName` come from
//     `window.kcreate.session.info()` when a session is live, or fall
//     back to a synthetic local id derived from the project id so
//     pre-session annotations still attribute to "this machine". The
//     bridge does not enforce the peer id matches anything — peer-id
//     attribution is informational, and the broadcast path
//     re-signs with the active session identity if/when one starts.
//
// Colour mapping:
//   * Re-uses `colorForPeer` from CursorOverlay so the same author
//     gets the same colour across every overlay (cursor, selection,
//     pin). Local-fallback uses the project-id-derived synthetic
//     peer-id so the local user's pins keep a consistent colour
//     before they ever start a session.

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import type {
  Annotation,
  AnnotationListResponse,
  ProjectInfo,
  SessionStartReport,
} from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

import { colorForPeer } from "./CursorOverlay";
import type { ViewportState } from "./CanvasHost";

/// Filter visible annotations by resolve state.
export type AnnotationVisibility = "open" | "resolved" | "all";

export interface AnnotationOverlayProps {
  /** Width / height of the parent canvas surface in CSS pixels. */
  width: number;
  height: number;
  /** Current viewport pan + zoom. Pin positions are world units. */
  viewport: ViewportState;
  /** UUID of the page the canvas is currently rendering. `null`
   * when no page is mounted (e.g. before the project finishes
   * loading). The overlay returns `null` in that case. */
  pageId: string | null;
  /** Optional project info — used to derive a stable synthetic
   * local-peer-id when no collab session is active so the author
   * attribution is consistent across the lifetime of the project. */
  project: ProjectInfo | null;
  /** When `false`, the overlay still renders existing pins (read
   * + thread interactions) but the double-click drop-pin gesture is
   * disabled. Lets the host gate the writing path behind a mode
   * toggle (`mode === "design"` etc.) without unmounting the whole
   * overlay. */
  allowCreate?: boolean;
  /** Forwarded so the host can hide tool overlays while a thread is
   * open. Optional. */
  onThreadOpenChange?: (open: boolean) => void;
}

/// Pin radius in screen-space pixels. Constant — does NOT scale
/// with `viewport.zoom` because the user always interacts with the
/// pin at the same visual size regardless of zoom level.
const PIN_RADIUS = 11;

/// Screen-space offset for the floating thread panel relative to
/// its anchor pin's centre. Picked so a pin at the canvas edge still
/// shows the panel inside the viewport for typical canvas widths.
const THREAD_OFFSET_X = 18;
const THREAD_OFFSET_Y = -8;

/// Width of the floating thread panel in CSS pixels.
const THREAD_PANEL_WIDTH = 280;

/// Convert a world-space (x, y) into screen coordinates using the
/// `screen = world * zoom + pan` projection. Same formula as
/// `projectWorld` in CursorOverlay (kept inline to avoid a circular
/// import — `CursorOverlay` exports the colour helper we depend on,
/// and re-exporting the projection through it would tangle the
/// dependency graph). The two implementations are pinned in lockstep
/// by a unit test in `__tests__/annotation_overlay_projection.test.ts`.
function projectWorld(
  worldX: number,
  worldY: number,
  viewport: ViewportState,
): { x: number; y: number } {
  return {
    x: worldX * viewport.zoom + viewport.panX,
    y: worldY * viewport.zoom + viewport.panY,
  };
}

/// Stable synthetic peer-id derived from a project id for the
/// solo-author case. Format: `local:<projectId>`. Matches the
/// `colorForPeer` palette mapping deterministically so the same
/// project always lands on the same colour. Exported for tests +
/// `EditorPage` annotation-creation paths that may want to attribute
/// outside of this component.
export function localPeerIdForProject(projectId: string): string {
  return `local:${projectId}`;
}

export function AnnotationOverlay({
  width,
  height,
  viewport,
  pageId,
  project,
  allowCreate = true,
  onThreadOpenChange,
}: AnnotationOverlayProps): JSX.Element | null {
  const [annotations, setAnnotations] = useState<Annotation[]>([]);
  const [openThreadId, setOpenThreadId] = useState<string | null>(null);
  const [visibility, setVisibility] = useState<AnnotationVisibility>("open");
  const [draftPosition, setDraftPosition] = useState<{
    x: number;
    y: number;
  } | null>(null);
  const [sessionIdentity, setSessionIdentity] =
    useState<SessionStartReport | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const overlayRef = useRef<SVGSVGElement | null>(null);

  // Notify host whenever the thread visibility transitions so it
  // can dim tool palettes. Skipping the closure when no callback was
  // supplied keeps the dependency list tight.
  useEffect(() => {
    onThreadOpenChange?.(openThreadId !== null || draftPosition !== null);
  }, [draftPosition, onThreadOpenChange, openThreadId]);

  // Fetch list whenever the page changes or the visibility filter
  // toggles. Wraps both filter axes (`include_resolved` /
  // `include_unresolved`) so the panel matches the bridge contract
  // exactly — see `crates/kcreate_bridge/src/annotation_bridge.rs`.
  const reload = useCallback(async (): Promise<void> => {
    if (pageId == null) {
      setAnnotations([]);
      return;
    }
    try {
      const includeResolved = visibility !== "open";
      const includeUnresolved = visibility !== "resolved";
      const resp: AnnotationListResponse =
        await window.kcreate.annotation.list({
          pageId,
          includeResolved,
          includeUnresolved,
        });
      setAnnotations(resp.annotations);
      setError(null);
    } catch (e) {
      setError(errMsg(e));
    }
  }, [pageId, visibility]);

  useEffect(() => {
    void reload();
  }, [reload]);

  // Pull the current session identity once on mount + on every
  // session lifecycle change. Used as the `authorPeerId` /
  // `authorName` for new annotations so attribution is consistent
  // with the cursor / selection overlays.
  useEffect(() => {
    let cancelled = false;
    const refresh = async (): Promise<void> => {
      try {
        const info = await window.kcreate.session.info();
        if (!cancelled) setSessionIdentity(info);
      } catch {
        // Session bridge unavailable (e.g. test env) — leave the
        // identity at its last value. The component will fall back
        // to the project-derived synthetic id below.
      }
    };
    void refresh();
    const unsubscribe = window.kcreate.session.onEvent((ev) => {
      if (ev.kind === "sessionStarted" || ev.kind === "sessionLeft") {
        void refresh();
      }
      // Re-pull annotations when a peer broadcasts a mutation that
      // touched our page. The bridge guarantees `pageIds` is the
      // exact set of affected page UUIDs — we check membership
      // rather than blindly reloading on every event so other
      // pages' churn doesn't trigger work.
      if (ev.kind === "annotationsApplied") {
        if (pageId != null && ev.pageIds.includes(pageId)) {
          void reload();
        }
      }
    });
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, [pageId, reload]);

  // Compute the canonical author tuple. Session > project fallback
  // > absolute fallback ("local:unknown" — used only in the test
  // harness where neither bridge is wired).
  const authorPeerId = useMemo(() => {
    if (sessionIdentity != null) return sessionIdentity.peerId;
    if (project != null) return localPeerIdForProject(project.id);
    return "local:unknown";
  }, [project, sessionIdentity]);
  const authorName = useMemo(() => {
    if (sessionIdentity != null && sessionIdentity.displayName.length > 0) {
      return sessionIdentity.displayName;
    }
    return "Me";
  }, [sessionIdentity]);

  // Map a screen-space pointer event into world-space using the
  // inverse projection. The pin position is canonically stored in
  // world units so a future zoom / pan re-projects cleanly.
  const eventToWorld = useCallback(
    (screenX: number, screenY: number): { x: number; y: number } => {
      const zoom = viewport.zoom === 0 ? 1 : viewport.zoom;
      return {
        x: (screenX - viewport.panX) / zoom,
        y: (screenY - viewport.panY) / zoom,
      };
    },
    [viewport.panX, viewport.panY, viewport.zoom],
  );

  const handleSurfaceDoubleClick = useCallback(
    (event: React.MouseEvent<SVGSVGElement>) => {
      if (!allowCreate || pageId == null) return;
      // Use the SVG element's own bounding rect to translate the
      // viewport-relative `clientX/Y` into the overlay's local
      // coordinate system. This handles the case where the overlay
      // is not glued to the viewport top-left (e.g. wrapped in a
      // panel with margins).
      const svg = overlayRef.current;
      if (svg == null) return;
      const rect = svg.getBoundingClientRect();
      const screenX = event.clientX - rect.left;
      const screenY = event.clientY - rect.top;
      const world = eventToWorld(screenX, screenY);
      setDraftPosition(world);
      // Close any open thread so the draft editor takes focus.
      setOpenThreadId(null);
    },
    [allowCreate, eventToWorld, pageId],
  );

  const handleCreate = useCallback(
    async (text: string): Promise<void> => {
      if (pageId == null || draftPosition == null) return;
      setBusy("create");
      try {
        await window.kcreate.annotation.create({
          pageId,
          authorPeerId,
          authorName,
          position: draftPosition,
          text,
        });
        setDraftPosition(null);
        await reload();
      } catch (e) {
        setError(errMsg(e));
      } finally {
        setBusy(null);
      }
    },
    [authorName, authorPeerId, draftPosition, pageId, reload],
  );

  const handleReply = useCallback(
    async (parentId: string, text: string): Promise<void> => {
      setBusy("reply");
      try {
        await window.kcreate.annotation.reply({
          parentId,
          authorPeerId,
          authorName,
          text,
        });
        await reload();
      } catch (e) {
        setError(errMsg(e));
      } finally {
        setBusy(null);
      }
    },
    [authorName, authorPeerId, reload],
  );

  const handleResolve = useCallback(
    async (id: string, resolved: boolean): Promise<void> => {
      setBusy(`resolve-${id}`);
      try {
        await window.kcreate.annotation.resolve({ id, resolved });
        await reload();
      } catch (e) {
        setError(errMsg(e));
      } finally {
        setBusy(null);
      }
    },
    [reload],
  );

  const handleDelete = useCallback(
    async (id: string): Promise<void> => {
      setBusy(`delete-${id}`);
      try {
        await window.kcreate.annotation.delete(id);
        if (openThreadId === id) setOpenThreadId(null);
        await reload();
      } catch (e) {
        setError(errMsg(e));
      } finally {
        setBusy(null);
      }
    },
    [openThreadId, reload],
  );

  // Group replies onto their thread roots so the floating panel can
  // render the full conversation in chronological order. The bridge
  // returns the head + all replies in one list; we just bucket.
  const threads = useMemo(() => groupIntoThreads(annotations), [annotations]);
  const visibleHeads = useMemo(
    () => threads.filter((t) => matchesVisibility(t.head, visibility)),
    [threads, visibility],
  );
  const openThread = useMemo(
    () => threads.find((t) => t.head.id === openThreadId) ?? null,
    [openThreadId, threads],
  );

  if (pageId == null) return null;

  return (
    <svg
      ref={overlayRef}
      // The SVG itself captures the double-click that drops a new
      // pin, but individual pin children opt out of bubbling via
      // `event.stopPropagation()` in their handlers so clicking an
      // existing pin opens the thread instead of dropping a new one.
      onDoubleClick={handleSurfaceDoubleClick}
      style={{
        position: "absolute",
        inset: 0,
        width,
        height,
        // Pins themselves take pointer events (see per-circle
        // override below). The surface is `auto` so the
        // double-click handler fires; if `allowCreate` is false we
        // turn this off so canvas tools below can receive the
        // gesture.
        pointerEvents: allowCreate ? "auto" : "none",
      }}
      width={width}
      height={height}
      role="presentation"
    >
      {visibleHeads.map(({ head }) => {
        const screen = projectWorld(head.position.x, head.position.y, viewport);
        const colour = colorForPeer(head.authorPeerId);
        const dimmed = head.resolved;
        return (
          <g
            key={head.id}
            transform={`translate(${screen.x}, ${screen.y})`}
            style={{ pointerEvents: "auto", cursor: "pointer" }}
            onClick={(e) => {
              e.stopPropagation();
              setOpenThreadId((id) => (id === head.id ? null : head.id));
              setDraftPosition(null);
            }}
            onDoubleClick={(e) => {
              // Swallow the double-click on the pin itself so the
              // SVG-level handler doesn't *also* drop a new pin on
              // top of the existing one.
              e.stopPropagation();
            }}
          >
            {/* Pin body. Slight white outline so it stays legible
                on any background. Opacity dims resolved threads
                without hiding them outright. */}
            <circle
              r={PIN_RADIUS}
              fill={colour}
              fillOpacity={dimmed ? 0.4 : 0.95}
              stroke="#ffffff"
              strokeWidth={2}
            />
            <text
              x={0}
              y={4}
              textAnchor="middle"
              fill="#ffffff"
              fontSize={11}
              fontWeight={700}
              fontFamily="-apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif"
              style={{ pointerEvents: "none", userSelect: "none" }}
            >
              {initialFor(head.authorName)}
            </text>
          </g>
        );
      })}
      {openThread != null
        ? (() => {
            const screen = projectWorld(
              openThread.head.position.x,
              openThread.head.position.y,
              viewport,
            );
            return (
              <ThreadPanel
                key={openThread.head.id}
                anchorX={screen.x + THREAD_OFFSET_X}
                anchorY={screen.y + THREAD_OFFSET_Y}
                head={openThread.head}
                replies={openThread.replies}
                isLocalAuthor={(a) => a.authorPeerId === authorPeerId}
                onReply={(text) => handleReply(openThread.head.id, text)}
                onResolve={(resolved) =>
                  handleResolve(openThread.head.id, resolved)
                }
                onDelete={(id) => handleDelete(id)}
                onClose={() => setOpenThreadId(null)}
                busy={busy}
              />
            );
          })()
        : null}
      {draftPosition != null
        ? (() => {
            const screen = projectWorld(
              draftPosition.x,
              draftPosition.y,
              viewport,
            );
            return (
              <DraftPanel
                key="draft"
                anchorX={screen.x + THREAD_OFFSET_X}
                anchorY={screen.y + THREAD_OFFSET_Y}
                onSubmit={(text) => handleCreate(text)}
                onCancel={() => setDraftPosition(null)}
                busy={busy === "create"}
              />
            );
          })()
        : null}
      <Legend
        visibility={visibility}
        onChange={setVisibility}
        canvasWidth={width}
        canvasHeight={height}
        error={error}
        onDismissError={() => setError(null)}
      />
    </svg>
  );
}

interface ThreadGroup {
  head: Annotation;
  replies: Annotation[];
}

function groupIntoThreads(list: Annotation[]): ThreadGroup[] {
  const heads = list.filter((a) => a.threadId == null);
  // Bucket replies by their thread id. The bridge's
  // `annotation_reply` walks parentId to the root so the
  // `threadId` field always points at the head — that means a
  // single index lookup is sufficient here.
  const repliesByThread = new Map<string, Annotation[]>();
  for (const a of list) {
    if (a.threadId == null) continue;
    const bucket = repliesByThread.get(a.threadId);
    if (bucket == null) {
      repliesByThread.set(a.threadId, [a]);
    } else {
      bucket.push(a);
    }
  }
  return heads
    .map((head) => {
      const replies = (repliesByThread.get(head.id) ?? []).slice();
      // Sort replies oldest-first by timestamp so the thread reads
      // chronologically. `Annotation.timestamp` is RFC3339 so
      // lexicographic compare is equivalent to chronological.
      replies.sort((a, b) => a.timestamp.localeCompare(b.timestamp));
      return { head, replies };
    })
    .sort(
      // Head sort: most-recent first so freshly created threads
      // appear at the top of the visible list (in case of an
      // overlap on screen).
      (a, b) => b.head.timestamp.localeCompare(a.head.timestamp),
    );
}

function matchesVisibility(
  head: Annotation,
  visibility: AnnotationVisibility,
): boolean {
  switch (visibility) {
    case "open":
      return !head.resolved;
    case "resolved":
      return head.resolved;
    case "all":
      return true;
    default:
      return true;
  }
}

function initialFor(name: string): string {
  const trimmed = name.trim();
  if (trimmed.length === 0) return "?";
  // Take the first grapheme via `Array.from` so a non-Latin
  // display name (CJK / emoji) still produces a single visible
  // glyph rather than half of a surrogate pair.
  const first = Array.from(trimmed)[0];
  return (first ?? "?").toUpperCase();
}

function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

// ---------------------------------------------------------------------
// Thread / draft / legend sub-components.
//
// Implemented as `<foreignObject>` islands inside the parent SVG so
// we can use ordinary HTML controls (textarea + buttons + scroll)
// without re-implementing them in SVG primitives.
// ---------------------------------------------------------------------

interface ThreadPanelProps {
  anchorX: number;
  anchorY: number;
  head: Annotation;
  replies: Annotation[];
  isLocalAuthor: (a: Annotation) => boolean;
  onReply: (text: string) => Promise<void> | void;
  onResolve: (resolved: boolean) => Promise<void> | void;
  onDelete: (id: string) => Promise<void> | void;
  onClose: () => void;
  busy: string | null;
}

function ThreadPanel({
  anchorX,
  anchorY,
  head,
  replies,
  isLocalAuthor,
  onReply,
  onResolve,
  onDelete,
  onClose,
  busy,
}: ThreadPanelProps): JSX.Element {
  const [replyDraft, setReplyDraft] = useState("");
  const scrollRef = useRef<HTMLDivElement>(null);

  // Auto-scroll the conversation list to the bottom whenever a new
  // reply lands so the latest message is visible. Without this the
  // user has to manually scroll after each reply.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el == null) return;
    el.scrollTop = el.scrollHeight;
  }, [replies.length]);

  return (
    <foreignObject
      x={anchorX}
      y={anchorY}
      width={THREAD_PANEL_WIDTH}
      height={360}
      style={{ overflow: "visible" }}
    >
      <div
        // Stop the parent SVG from interpreting clicks inside the
        // floating panel as canvas double-click / pin clicks.
        onMouseDown={(e) => e.stopPropagation()}
        onClick={(e) => e.stopPropagation()}
        onDoubleClick={(e) => e.stopPropagation()}
        style={{
          background: colors.bg,
          border: `1px solid ${colors.border}`,
          borderRadius: radius.card,
          boxShadow: "0 4px 12px rgba(0,0,0,0.15)",
          width: THREAD_PANEL_WIDTH,
          display: "flex",
          flexDirection: "column",
          padding: spacing.sm,
          gap: spacing.xs,
        }}
      >
        <header
          style={{
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
            gap: spacing.xs,
          }}
        >
          <strong style={{ fontSize: 12, color: colors.text }}>
            {head.authorName}
          </strong>
          <div style={{ display: "flex", gap: 6 }}>
            <button
              type="button"
              onClick={() => {
                void onResolve(!head.resolved);
              }}
              disabled={busy != null}
              style={pillButtonStyle(head.resolved ? "warn" : "accent")}
            >
              {head.resolved ? "Reopen" : "Resolve"}
            </button>
            {isLocalAuthor(head) ? (
              <button
                type="button"
                onClick={() => {
                  void onDelete(head.id);
                }}
                disabled={busy != null}
                style={pillButtonStyle("danger")}
              >
                Delete
              </button>
            ) : null}
            <button
              type="button"
              onClick={onClose}
              aria-label="Close thread"
              style={pillButtonStyle("plain")}
            >
              ✕
            </button>
          </div>
        </header>
        <div
          ref={scrollRef}
          style={{
            maxHeight: 220,
            overflowY: "auto",
            display: "flex",
            flexDirection: "column",
            gap: spacing.xs,
          }}
        >
          <Bubble annotation={head} isLocal={isLocalAuthor(head)} />
          {replies.map((reply) => (
            <Bubble
              key={reply.id}
              annotation={reply}
              isLocal={isLocalAuthor(reply)}
              onDelete={
                isLocalAuthor(reply) ? () => void onDelete(reply.id) : undefined
              }
            />
          ))}
        </div>
        <ReplyInput
          value={replyDraft}
          onChange={setReplyDraft}
          onSubmit={async () => {
            const text = replyDraft.trim();
            if (text.length === 0) return;
            await onReply(text);
            setReplyDraft("");
          }}
          busy={busy === "reply"}
        />
        <small style={{ fontSize: 10, color: colors.textMuted }}>
          {formatTimestamp(head.timestamp)}
        </small>
      </div>
    </foreignObject>
  );
}

function Bubble({
  annotation,
  isLocal,
  onDelete,
}: {
  annotation: Annotation;
  isLocal: boolean;
  onDelete?: () => void;
}): JSX.Element {
  const colour = colorForPeer(annotation.authorPeerId);
  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        background: isLocal ? colors.accentBgSoft : colors.bgSoft,
        border: `1px solid ${colors.border}`,
        borderRadius: radius.md,
        padding: spacing.xs,
        gap: 2,
      }}
    >
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          gap: 4,
        }}
      >
        <span
          style={{
            fontSize: 10,
            fontWeight: 600,
            color: colour,
          }}
        >
          {annotation.authorName}
        </span>
        {onDelete != null ? (
          <button
            type="button"
            onClick={onDelete}
            aria-label="Delete reply"
            style={{
              border: "none",
              background: "transparent",
              color: colors.textMuted,
              fontSize: 10,
              cursor: "pointer",
              padding: 0,
            }}
          >
            ✕
          </button>
        ) : null}
      </div>
      <p
        style={{
          margin: 0,
          fontSize: 12,
          color: colors.text,
          whiteSpace: "pre-wrap",
          wordBreak: "break-word",
        }}
      >
        {annotation.text}
      </p>
      <small style={{ fontSize: 9, color: colors.textMuted }}>
        {formatTimestamp(annotation.timestamp)}
      </small>
    </div>
  );
}

interface DraftPanelProps {
  anchorX: number;
  anchorY: number;
  onSubmit: (text: string) => Promise<void> | void;
  onCancel: () => void;
  busy: boolean;
}

function DraftPanel({
  anchorX,
  anchorY,
  onSubmit,
  onCancel,
  busy,
}: DraftPanelProps): JSX.Element {
  const [draft, setDraft] = useState("");
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // Auto-focus on mount so the user can start typing immediately
  // after double-clicking to drop a pin.
  useEffect(() => {
    textareaRef.current?.focus();
  }, []);

  return (
    <foreignObject
      x={anchorX}
      y={anchorY}
      width={THREAD_PANEL_WIDTH}
      height={180}
      style={{ overflow: "visible" }}
    >
      <div
        onMouseDown={(e) => e.stopPropagation()}
        onClick={(e) => e.stopPropagation()}
        onDoubleClick={(e) => e.stopPropagation()}
        style={{
          background: colors.bg,
          border: `1px solid ${colors.border}`,
          borderRadius: radius.card,
          boxShadow: "0 4px 12px rgba(0,0,0,0.15)",
          width: THREAD_PANEL_WIDTH,
          display: "flex",
          flexDirection: "column",
          padding: spacing.sm,
          gap: spacing.xs,
        }}
      >
        <strong style={{ fontSize: 12, color: colors.text }}>New note</strong>
        <textarea
          ref={textareaRef}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          rows={4}
          placeholder="Describe the change to discuss…"
          style={textareaStyle}
        />
        <div style={{ display: "flex", gap: 6, justifyContent: "flex-end" }}>
          <button
            type="button"
            onClick={onCancel}
            disabled={busy}
            style={pillButtonStyle("plain")}
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={() => {
              const text = draft.trim();
              if (text.length === 0) return;
              void onSubmit(text);
            }}
            disabled={busy || draft.trim().length === 0}
            style={pillButtonStyle("accent")}
          >
            {busy ? "Posting…" : "Post"}
          </button>
        </div>
      </div>
    </foreignObject>
  );
}

interface ReplyInputProps {
  value: string;
  onChange: (v: string) => void;
  onSubmit: () => Promise<void> | void;
  busy: boolean;
}

function ReplyInput({
  value,
  onChange,
  onSubmit,
  busy,
}: ReplyInputProps): JSX.Element {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      <textarea
        value={value}
        onChange={(e) => onChange(e.target.value)}
        rows={2}
        placeholder="Reply…"
        style={textareaStyle}
        onKeyDown={(e) => {
          // Cmd/Ctrl+Enter posts the reply — matches the convention
          // of GitHub / Slack / Linear so it's not a surprise.
          if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
            e.preventDefault();
            void onSubmit();
          }
        }}
      />
      <div style={{ display: "flex", justifyContent: "flex-end" }}>
        <button
          type="button"
          onClick={() => {
            void onSubmit();
          }}
          disabled={busy || value.trim().length === 0}
          style={pillButtonStyle("accent")}
        >
          {busy ? "Posting…" : "Reply"}
        </button>
      </div>
    </div>
  );
}

interface LegendProps {
  visibility: AnnotationVisibility;
  onChange: (v: AnnotationVisibility) => void;
  canvasWidth: number;
  canvasHeight: number;
  error: string | null;
  onDismissError: () => void;
}

function Legend({
  visibility,
  onChange,
  canvasWidth,
  canvasHeight,
  error,
  onDismissError,
}: LegendProps): JSX.Element {
  return (
    <foreignObject
      x={Math.max(0, canvasWidth - 220)}
      y={Math.max(0, canvasHeight - 60)}
      width={210}
      height={56}
      style={{ overflow: "visible" }}
    >
      <div
        onMouseDown={(e) => e.stopPropagation()}
        onClick={(e) => e.stopPropagation()}
        onDoubleClick={(e) => e.stopPropagation()}
        style={{
          background: "rgba(17, 24, 39, 0.7)",
          color: colors.textInverse,
          borderRadius: radius.md,
          padding: "4px 8px",
          display: "flex",
          flexDirection: "column",
          gap: 2,
          fontSize: 11,
        }}
      >
        <div style={{ display: "flex", gap: 4, alignItems: "center" }}>
          <span style={{ opacity: 0.8 }}>Notes:</span>
          {(["open", "resolved", "all"] as const).map((v) => (
            <button
              key={v}
              type="button"
              onClick={() => onChange(v)}
              style={{
                background: visibility === v ? colors.accent : "transparent",
                color: visibility === v ? colors.textInverse : colors.textInverse,
                border: `1px solid ${visibility === v ? colors.accent : "rgba(255,255,255,0.3)"}`,
                borderRadius: radius.pill,
                padding: "1px 6px",
                fontSize: 10,
                fontWeight: 600,
                textTransform: "capitalize",
                cursor: "pointer",
              }}
            >
              {v}
            </button>
          ))}
        </div>
        {error != null ? (
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 4,
              fontSize: 10,
              color: colors.dangerOverlay,
            }}
          >
            <span>Error: {error}</span>
            <button
              type="button"
              onClick={onDismissError}
              style={{
                background: "transparent",
                border: "none",
                color: colors.textInverse,
                cursor: "pointer",
                fontSize: 10,
                padding: 0,
              }}
            >
              ✕
            </button>
          </div>
        ) : null}
      </div>
    </foreignObject>
  );
}

function formatTimestamp(iso: string): string {
  // RFC3339 → human-readable local time. Defensive parse: if the
  // string fails to parse (e.g. malformed payload from a future
  // schema), fall back to the raw string so the user at least sees
  // something rather than "Invalid Date".
  try {
    const dt = new Date(iso);
    if (Number.isNaN(dt.getTime())) return iso;
    return dt.toLocaleString(undefined, {
      month: "short",
      day: "numeric",
      hour: "numeric",
      minute: "2-digit",
    });
  } catch {
    return iso;
  }
}

const textareaStyle = {
  fontFamily: "inherit",
  fontSize: 12,
  padding: 6,
  borderRadius: radius.sm,
  border: `1px solid ${colors.border}`,
  background: colors.bg,
  color: colors.text,
  width: "100%",
  boxSizing: "border-box",
  resize: "vertical",
} as const;

function pillButtonStyle(
  kind: "accent" | "danger" | "warn" | "plain",
): React.CSSProperties {
  const base: React.CSSProperties = {
    padding: "2px 8px",
    fontSize: 10,
    fontWeight: 600,
    borderRadius: radius.pill,
    cursor: "pointer",
    border: "none",
  };
  switch (kind) {
    case "accent":
      return { ...base, background: colors.accent, color: colors.textInverse };
    case "danger":
      return { ...base, background: colors.danger, color: colors.textInverse };
    case "warn":
      return { ...base, background: colors.warn, color: colors.textInverse };
    case "plain":
    default:
      return {
        ...base,
        background: "transparent",
        color: colors.textMuted,
        border: `1px solid ${colors.border}`,
      };
  }
}

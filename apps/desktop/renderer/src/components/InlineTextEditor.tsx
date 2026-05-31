// InlineTextEditor — Phase A1 canvas double-click editor.
//
// Floats a `contenteditable` div over a hit-tested `TextLayer`
// node's bounding box, styled to mirror the resolved shaped
// glyphs (font family, size, line height). On blur the buffer is
// committed via `window.kcreate.text.replaceRange(0, initial.length,
// next)`; the call uses UTF-16 indices, matching JavaScript's
// native string length / `Selection.anchorOffset` semantics, so we
// don't need to count code points on the renderer side.
//
// The editor receives the *screen-space* rectangle from the host
// (already projected through the viewport). It does not re-read
// pan/zoom — keeping the projection in EditorPage means the
// editor never disagrees with the canvas underneath when a peer
// mutates the viewport while the user is typing.

import { useEffect, useLayoutEffect, useRef } from "react";

import type { TextStyleWire } from "../../../shared/scene";
import { colors, radius } from "../styles/tokens";

export interface InlineTextEditorProps {
  /// Document-graph id of the `TextLayer` being edited. Forwarded
  /// to the `replaceRange` commit call on blur.
  nodeId: string;
  /// Screen-space rectangle (already projected through the host's
  /// viewport). The editor mounts at exactly this position so it
  /// overlays the canvas glyphs 1:1.
  rect: { x: number; y: number; width: number; height: number };
  /// Style fields the shaper actually applies. Mirrored into the
  /// editor's CSS so the typed glyphs match the rendered ones.
  style: TextStyleWire;
  /// The node's text content at the moment editing started. Used
  /// to (a) hydrate the editor's initial buffer and (b) compute the
  /// UTF-16 range passed to `replaceRange` on commit.
  initialContent: string;
  /// Called when the user accepts the edit (blur or Enter without
  /// Shift). Receives the next content; the host is expected to
  /// commit via `text.replaceRange(...)`.
  onCommit: (next: string) => void;
  /// Called when the user dismisses the edit (Escape). The buffer
  /// is discarded; the host is expected to unmount this component.
  onCancel: () => void;
}

export function InlineTextEditor({
  rect,
  style,
  initialContent,
  onCommit,
  onCancel,
}: InlineTextEditorProps): JSX.Element {
  const ref = useRef<HTMLDivElement | null>(null);

  // Mount-time effects: seed the buffer + focus + select-all so the
  // user can immediately overtype. `useLayoutEffect` runs before
  // paint, which avoids a 1-frame flash of empty content.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.textContent = initialContent;
    el.focus();
    // Place caret at end (matches the implicit "click to edit" UX
    // where the user usually wants to append, not overtype).
    const sel = window.getSelection();
    if (sel) {
      const range = document.createRange();
      range.selectNodeContents(el);
      range.collapse(false);
      sel.removeAllRanges();
      sel.addRange(range);
    }
  }, [initialContent]);

  // Keyboard shortcuts. Enter without Shift commits; Shift+Enter
  // inserts a newline (default browser behaviour, so we don't have
  // to do anything). Escape cancels.
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const onKeyDown = (e: KeyboardEvent) => {
      // Stop the parent canvas from interpreting keys (e.g. `V` for
      // select tool, `T` for text tool) while the user is typing.
      e.stopPropagation();
      if (e.key === "Escape") {
        e.preventDefault();
        onCancel();
        return;
      }
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        // Blur to fire the commit path; keeps the codepath single.
        el.blur();
      }
    };
    el.addEventListener("keydown", onKeyDown);
    return () => el.removeEventListener("keydown", onKeyDown);
  }, [onCancel]);

  return (
    <div
      ref={ref}
      contentEditable
      suppressContentEditableWarning
      // Pointer events must be enabled so the user can drag-select
      // text inside the editor; the surrounding overlay is
      // `pointer-events: none`, so without this prop the editor
      // would be unfocusable.
      onPointerDown={(e) => e.stopPropagation()}
      onBlur={(e) => {
        const next = e.currentTarget.textContent ?? "";
        if (next === initialContent) {
          // No-op edit; cancel so the host clears the overlay
          // without churning the operation log.
          onCancel();
          return;
        }
        onCommit(next);
      }}
      style={{
        position: "absolute",
        left: rect.x,
        top: rect.y,
        width: rect.width,
        height: rect.height,
        minWidth: 32,
        minHeight: style.fontSize * style.lineHeight,
        fontFamily: style.fontFamily,
        fontSize: style.fontSize,
        lineHeight: style.lineHeight,
        // Match canvas glyph rendering hints: opaque background so
        // the user sees a clean text field while editing, but
        // bordered subtly so it doesn't visually clash with the
        // canvas. The accent outline marks the editor as "active".
        background: colors.bg,
        color: colors.text,
        outline: `1px solid ${colors.accent}`,
        outlineOffset: 2,
        padding: "2px 4px",
        boxSizing: "border-box",
        borderRadius: radius.card,
        whiteSpace: "pre-wrap",
        wordBreak: "break-word",
        overflow: "auto",
        zIndex: 50,
        // Suppress the browser's default focus ring; the accent
        // outline above is our focus indicator.
        WebkitUserSelect: "text",
        userSelect: "text",
      }}
      data-testid="inline-text-editor"
    />
  );
}

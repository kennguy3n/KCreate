import { useEffect, useRef } from "react";

// Elements that can receive keyboard focus inside a trapped container.
const FOCUSABLE_SELECTOR = [
  "a[href]",
  "button:not([disabled])",
  "textarea:not([disabled])",
  'input:not([disabled]):not([type="hidden"])',
  "select:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
].join(",");

function focusableWithin(container: HTMLElement): HTMLElement[] {
  return Array.from(
    container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR),
  ).filter(
    (el) =>
      el.offsetWidth > 0 ||
      el.offsetHeight > 0 ||
      el === document.activeElement,
  );
}

export interface FocusTrapOptions {
  /** Whether the trap is engaged (e.g. the modal is open). */
  readonly active: boolean;
  /** Invoked when Escape is pressed while the trap is active. */
  readonly onEscape?: () => void;
}

/**
 * Confine keyboard focus to a container while `active`, and restore it
 * afterwards — the focus-management contract every modal/overlay owes:
 *
 *   * on activate, focus moves into the container (first focusable, or
 *     the container itself);
 *   * Tab / Shift+Tab wrap at the focusable boundary so focus can never
 *     escape behind the overlay;
 *   * Escape calls `onEscape`;
 *   * on deactivate/unmount, focus returns to whatever was focused
 *     before the trap engaged.
 *
 * Returns a ref to spread onto the container element.
 */
export function useFocusTrap<T extends HTMLElement = HTMLElement>({
  active,
  onEscape,
}: FocusTrapOptions): React.RefObject<T> {
  const containerRef = useRef<T>(null);
  const previouslyFocused = useRef<HTMLElement | null>(null);
  // Keep the latest onEscape without re-installing the key listener.
  const onEscapeRef = useRef(onEscape);
  onEscapeRef.current = onEscape;

  useEffect(() => {
    if (!active) return;
    const container = containerRef.current;
    if (!container) return;

    previouslyFocused.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;

    // Move focus in, unless something inside already owns it (e.g. an
    // input the component focused itself on open).
    if (!container.contains(document.activeElement)) {
      const focusables = focusableWithin(container);
      (focusables[0] ?? container).focus();
    }

    function onKeyDown(event: KeyboardEvent): void {
      if (event.key === "Escape") {
        onEscapeRef.current?.();
        return;
      }
      if (event.key !== "Tab") return;
      const node = containerRef.current;
      if (!node) return;
      const focusables = focusableWithin(node);
      const first = focusables[0];
      const last = focusables[focusables.length - 1];
      if (!first || !last) {
        // Nothing focusable inside — keep focus pinned to the container.
        event.preventDefault();
        node.focus();
        return;
      }
      const activeEl = document.activeElement;
      if (event.shiftKey) {
        if (activeEl === first || !node.contains(activeEl)) {
          event.preventDefault();
          last.focus();
        }
      } else if (activeEl === last || !node.contains(activeEl)) {
        event.preventDefault();
        first.focus();
      }
    }

    document.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.removeEventListener("keydown", onKeyDown, true);
      // Restore focus to the opener on close/unmount.
      const toRestore = previouslyFocused.current;
      if (toRestore && document.contains(toRestore)) {
        toRestore.focus();
      }
    };
  }, [active]);

  return containerRef;
}

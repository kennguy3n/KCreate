import type { CSSProperties } from "react";

// Standard "visually hidden" recipe: the element stays in the
// accessibility tree (so screen readers announce it) but is clipped to
// a 1×1 box and removed from the visual layout. Used for off-screen
// labels and `aria-live` announcement regions.
export const visuallyHidden: CSSProperties = {
  position: "absolute",
  width: 1,
  height: 1,
  padding: 0,
  margin: -1,
  overflow: "hidden",
  clip: "rect(0, 0, 0, 0)",
  whiteSpace: "nowrap",
  border: 0,
};

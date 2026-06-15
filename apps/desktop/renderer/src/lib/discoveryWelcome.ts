// First-run discovery-welcome gating. The welcome overlay points a
// brand-new user at the headline flows (templates, AI generate,
// elements) and the command palette. We only ever want to show it
// once, so the "seen" flag is persisted to localStorage rather than
// to bridge preferences: it is a pure renderer-UI nicety with no
// document or account semantics, and keeping it client-side avoids an
// IPC round-trip on every editor mount.
//
// This is intentionally separate from the bridge-backed `WelcomeModal`
// (the AI-model installer gated on `preferences.onboarding.completed`)
// — that overlay handles a different concern (downloading a local
// model) and lives on the HomePage.
//
// Privacy: a single boolean-ish marker is stored; no document content,
// no search text, nothing that leaves localStorage.

export const DISCOVERY_WELCOME_STORAGE_KEY = "kcreate.welcome.v1";

// Stored value when the user has seen/dismissed the welcome. The exact
// string is irrelevant to readers (presence is what matters) but we
// keep it stable + meaningful for anyone inspecting localStorage.
const SEEN_VALUE = "seen";

/**
 * Whether the first-run discovery welcome should be shown. Returns
 * `false` once the user has dismissed it (or taken any action from
 * it), and `false` when localStorage is unavailable (private mode /
 * disabled) so we never trap the user behind an overlay we can't
 * remember dismissing.
 */
export function shouldShowDiscoveryWelcome(): boolean {
  if (typeof window === "undefined" || !window.localStorage) return false;
  try {
    return window.localStorage.getItem(DISCOVERY_WELCOME_STORAGE_KEY) === null;
  } catch {
    // Disabled storage — treat as "already seen" so a user who can't
    // persist the dismissal isn't shown the overlay on every mount.
    return false;
  }
}

/**
 * Persist that the user has seen the discovery welcome so it never
 * reappears. Swallows storage failures: the worst case is the welcome
 * shows again next launch, which is preferable to throwing into the
 * dismiss handler.
 */
export function markDiscoveryWelcomeSeen(): void {
  if (typeof window === "undefined" || !window.localStorage) return;
  try {
    window.localStorage.setItem(DISCOVERY_WELCOME_STORAGE_KEY, SEEN_VALUE);
  } catch {
    // See docstring — non-fatal.
  }
}

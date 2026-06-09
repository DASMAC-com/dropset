// Lightweight UA-string mobile check. Client-only: returns false on the
// server so any branch that conditions on it renders the desktop path during
// SSR and resolves to the real value after hydration. We use this only for UX
// affordances (e.g. an "Install app" hint vs. an extension install link) where
// the worst-case hydration mismatch is a single row's icon and copy. If we
// ever need it for layout decisions, switch to a hydration-safe pattern.
//
// Regex covers the cases we actually care about for the wallet picker: iOS
// Safari (iPhone, iPad, iPod), Android browsers, and the in-app browsers
// (MetaMask, Phantom, etc.) that mostly ship with one of those UA tokens.

const MOBILE_UA_PATTERN =
  /Android|webOS|iPhone|iPad|iPod|BlackBerry|IEMobile|Opera Mini/i;

/**
 * Heuristic check for a mobile user agent. Returns `false` on the server.
 * Don't use this for anything that affects bundle splitting or first-paint
 * layout — only for client-side UX affordances after hydration.
 */
export function isMobile(): boolean {
  if (typeof navigator === "undefined") return false;
  return MOBILE_UA_PATTERN.test(navigator.userAgent);
}

"use client";

import { useEffect, useState } from "react";
import { isPresenterMode } from "@/lib/presenterMode";

/**
 * A way out of the presenter view, rendered over Spectacle's own chrome.
 *
 * The deck index sends most readers straight into presenter mode, so the first
 * thing they meet is the talk track — and, before this, no visible way back to
 * the slides on their own. Spectacle's ⌘⇧P has always done it, but a shortcut
 * nobody is told about is not an affordance, and its `PresenterMode` component
 * takes no children and exposes no slot to put one in. Hence an overlay.
 *
 * It is positioned against that view's fixed top-left geometry — a 60px logo
 * inset by 15px, with the banner text starting at 167px — which is stable
 * across viewport sizes because both are absolute pixel values in Spectacle's
 * own layout, not fractions of the column. `globals.css` hides the banner that
 * would otherwise sit here.
 *
 * Leaving reuses Spectacle's own exit: assigning `window.location.search`
 * drops the mode while keeping `slideIndex`/`stepIndex`, so the reader lands on
 * the slide they were reading rather than back at the title. The full
 * navigation is the point — the mode is read once, at mount.
 */
export function PresenterNotesExit() {
  const [inPresenterMode, setInPresenterMode] = useState(false);

  // Client-only read. The route is already client-rendered, but resolving this
  // in an effect keeps the first paint identical either way rather than
  // resting on that.
  useEffect(() => {
    setInPresenterMode(isPresenterMode(window.location.search));
  }, []);

  if (!inPresenterMode) return null;

  const hideNotes = () => {
    const current = new URLSearchParams(window.location.search);
    const kept = new URLSearchParams();
    for (const key of ["slideIndex", "stepIndex"]) {
      const value = current.get(key);
      if (value !== null) kept.set(key, value);
    }
    window.location.search = kept.toString();
  };

  return (
    <button
      type="button"
      onClick={hideNotes}
      title="Show the slides on their own — ⌘⇧P brings the notes back"
      className="fixed top-[30px] left-[90px] z-50 cursor-pointer rounded-full border border-white/25 bg-white/10 px-3 py-1.5 font-sans text-sm text-white backdrop-blur transition-colors hover:border-white/40 hover:bg-white/20"
    >
      Hide presenter notes
    </button>
  );
}

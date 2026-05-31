// Shared Radix dialog chrome so modals line up consistently. Both the swap
// token picker and the vault position manager anchor near the top of the
// viewport (rather than vertically centered) and share the blurred overlay.

export const DIALOG_OVERLAY_CLASS =
  "fixed inset-0 z-[60] bg-background/80 backdrop-blur-2xl";

// Top-anchored, horizontally centered. Callers append their own width /
// rounding / overflow. Pair with overflow-y handling on tall content.
export const DIALOG_CONTENT_POSITION =
  "-translate-x-1/2 fixed top-6 left-1/2 z-[70] max-h-[calc(100vh-3rem)]";

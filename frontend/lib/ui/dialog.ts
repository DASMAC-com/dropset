// Shared Radix dialog chrome so modals line up consistently. Both the swap
// token picker and the vault position manager anchor near the top of the
// viewport (rather than vertically centered) and share the blurred overlay.

// Named z-index scale for stacked overlays, so the order is intentional rather
// than a scatter of bare `z-[NN]` literals. Each tier sits above the previous:
// a dialog's overlay/content, a popover opened from within a dialog (e.g. the
// balance Max/% picker), then tooltips above everything.
export const Z_OVERLAY = "z-[60]";
export const Z_DIALOG = "z-[70]";
export const Z_POPOVER = "z-[80]";
export const Z_TOOLTIP = "z-[110]";

export const DIALOG_OVERLAY_CLASS = `fixed inset-0 ${Z_OVERLAY} bg-background/80 backdrop-blur-2xl`;

// Top-anchored, horizontally centered. Callers append their own width /
// rounding / overflow. Pair with overflow-y handling on tall content.
export const DIALOG_CONTENT_POSITION = `-translate-x-1/2 fixed top-6 left-1/2 ${Z_DIALOG} max-h-[calc(100vh-3rem)]`;

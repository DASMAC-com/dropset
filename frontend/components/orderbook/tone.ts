// The soft red/green pair shared by the depth ladder and the fills tape, so the
// two panes read as one instrument.
//
// Named by color rather than by meaning on purpose: the ladder colors a
// *level* (asks red, bids green) while the tape colors a *taker side* (buys
// green, sells red), so an ask-side fill is green even though the ask level it
// consumed was red. Each pane maps its own semantics onto these.
export const RED = "#ff6b81";
export const GREEN = "#3fd39b";

// Low-alpha washes of the two tones, for the ladder's depth bar and the
// update-flash overlay.
export const RED_BAR = "rgba(255,107,129,0.12)";
export const RED_FLASH = "rgba(255,107,129,0.30)";
export const GREEN_BAR = "rgba(63,211,155,0.12)";
export const GREEN_FLASH = "rgba(63,211,155,0.30)";

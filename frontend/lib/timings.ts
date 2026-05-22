// Centralized timing / capacity constants. Keep values that get tuned
// together close together — when a value changes, the surrounding context
// (and any other constants that depend on it) is easier to spot.

// ───────────── DFlow ─────────────

// Idle window after an input change before the quote fetch fires. Keeps
// typing from emitting one request per keystroke.
export const QUOTE_DEBOUNCE_MS = 500;
// Auto-refresh cadence after a successful fetch. 2 s gives a fresh route
// view while leaving plenty of bucket headroom for typing bursts.
export const QUOTE_REFRESH_MS = 2_000;

// DFlow's developer endpoint uses a token-bucket rate limiter
// (capacity 60, refill ~1/sec). Capacity-refill is documented at
// https://docs.dflow.net (dev endpoint); refill is empirical.
export const DFLOW_BUCKET_CAPACITY = 60;
export const DFLOW_REFILL_PER_SEC = 1;

// Defensive floor for projected `remaining` tokens. Drop below this and
// the timer defers another cycle rather than risk a 429.
export const MIN_TOKENS_TO_FETCH = 3;
// Hold off until projected remaining reaches this many tokens after a 429.
export const RECOVERY_TOKEN_TARGET = 10;

// Swap-confirmation polling.
export const SWAP_CONFIRMATION_TIMEOUT_MS = 60_000;
export const SWAP_CONFIRMATION_POLL_MS = 500;
// Tolerated consecutive nulls (RPC has never seen the signature) before
// declaring the tx dropped instead of polling to timeout.
export const SWAP_CONFIRM_MAX_UNKNOWN_POLLS = 10;

// ───────────── Jupiter / pricing ─────────────

// 10 s refresh cadence — empirically matches Jupiter's own server-side
// update rate for /tokens/v2/search.
export const TOKEN_INFO_REFRESH_MS = 10_000;
// TTL is kept at half the refresh interval so the boundary tick is
// never skipped by the dedupe check.
export const TOKEN_INFO_TTL_MS = 5_000;

// ───────────── Balances ─────────────

// Delay between the immediate post-swap balance fetch and a follow-up
// refresh. Absorbs RPC propagation lag between confirmation status and
// account state.
export const BALANCE_REFETCH_DELAY_MS = 1_500;

// ───────────── UI feedback ─────────────

// How long the clipboard-copy "Copied!" feedback stays on screen.
export const COPY_FEEDBACK_DURATION_MS = 1_500;

// Background flash duration in useFlashOnChange. Layered with NumberFlow
// to highlight which cell just updated on a refresh tick.
export const FLASH_DURATION_MS = 1_000;

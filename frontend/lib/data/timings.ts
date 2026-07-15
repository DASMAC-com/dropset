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

// Outer-edge timeout for the /order fetch. Long enough to absorb a slow
// quote-time route build, short enough that a hung endpoint surfaces as a
// retryable error rather than sticking the UI in "Preparing swap…".
export const DFLOW_ORDER_TIMEOUT_MS = 20_000;

// ───────────── Jupiter / pricing ─────────────

// 10 s refresh cadence — empirically matches Jupiter's own server-side
// update rate for /tokens/v2/search.
export const TOKEN_INFO_REFRESH_MS = 10_000;
// TTL is kept at half the refresh interval so the boundary tick is
// never skipped by the dedupe check.
export const TOKEN_INFO_TTL_MS = 5_000;
// Hard cap on every Jupiter token-info fetch. Long enough to ride out an
// occasional slow response, short enough that a hung endpoint can't pin
// the in-flight cache slot for the lifetime of the page.
export const JUPITER_FETCH_TIMEOUT_MS = 10_000;

// ───────────── Balances ─────────────

// Delay between the immediate post-swap balance fetch and a follow-up
// refresh. Absorbs RPC propagation lag between confirmation status and
// account state.
export const BALANCE_REFETCH_DELAY_MS = 1_500;

// ───────────── Order book ─────────────

// Live-poll cadence for the on-chain order-book viz. One getAccountInfo +
// getSlot per tick against the local (or mainnet) RPC. 1 s reads as live —
// the maker bot's flashed depth appears within a tick — without hammering
// the node the way the alpha viz's 500 ms poll did.
export const ORDER_BOOK_REFRESH_MS = 1_000;

// ───────────── UI feedback ─────────────

// How long the clipboard-copy "Copied!" feedback stays on screen.
export const COPY_FEEDBACK_DURATION_MS = 1_500;

// Background flash duration in useFlashOnChange. Layered with NumberFlow
// to highlight which cell just updated on a refresh tick.
export const FLASH_DURATION_MS = 1_000;

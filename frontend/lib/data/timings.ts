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
// getSlot + getBlockTime per tick against the local (or mainnet) RPC (expiry
// is dual-domain, and both halves are read from the chain — see
// lib/eclob/chainClock.ts). 1 s reads as live —
// the maker bot's flashed depth appears within a tick — without hammering
// the node the way the alpha viz's 500 ms poll did.
export const ORDER_BOOK_REFRESH_MS = 1_000;

// ───────────── Expiry gate clock ─────────────

// How far the visitor's device clock may sit from cluster time before the
// book is gated on the chain-derived estimate instead (lib/eclob/chainClock).
// Sized between the two error scales it separates: an NTP-synced device is
// within a second or so, and `getBlockTime` — a stake-weighted mean of vote
// timestamps — carries noise of its own, while the skew actually worth
// correcting runs to tens of seconds. Small against the top tier's lifetime,
// so a tolerated offset can't meaningfully misjudge a level.
export const CLOCK_SKEW_TOLERANCE_SECS = 5;

// Forward nudge applied to the gate whichever clock it ends up using, so a
// level inside its last moments is dropped here rather than quoted and then
// dropped by the engine mid-swap. Covers one poll tick plus the RPC round-trip
// that follows it. The two directions are not symmetric — briefly
// under-showing a dying level costs a sliver of displayed depth, while
// over-showing one costs the taker a soft revert plus fees.
export const CLOCK_SAFETY_MARGIN_SECS = 2;

// ───────────── Recent fills ─────────────

// Backoff before re-subscribing after the fills websocket drops or errors.
// Long enough not to hot-loop against a validator that's still starting (the
// localnet case), short enough that the tape resumes within a beat of the node
// coming back.
export const RECENT_FILLS_RESUBSCRIBE_MS = 2_000;

// ───────────── eCLOB availability ─────────────

// Backoff before re-probing whether a market exists for a pair, after a probe
// failed outright (as opposed to answering "no market"). Same localnet reason
// as the fills backoff above: `make demo` brings the frontend up before the
// validator, so the first probe can fail purely because nothing is listening
// yet — and a definitive answer is cached, so only the failures retry.
export const ECLOB_AVAILABILITY_RETRY_MS = 2_000;

// ───────────── UI feedback ─────────────

// How long the clipboard-copy "Copied!" feedback stays on screen.
export const COPY_FEEDBACK_DURATION_MS = 1_500;

// Background flash duration in useFlashOnChange. Layered with NumberFlow
// to highlight which cell just updated on a refresh tick.
export const FLASH_DURATION_MS = 1_000;

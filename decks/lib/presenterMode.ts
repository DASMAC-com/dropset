/**
 * The one place that knows how presenter mode is spelled in a deck's URL.
 *
 * Two sides depend on that spelling and they used to state it separately: the
 * index writes the query string onto each deck link, and the in-deck exit
 * button reads it back to decide whether to render at all. A contract split
 * across two files admits a silent failure — rewriting the produced value to
 * another form Spectacle accepts still opens the presenter view, while the way
 * out of it quietly disappears — and this package has no test runner to catch
 * that. One module, two exports, both sides importing them.
 */

/** Spectacle's own parameter name; its ⌘⇧P shortcut writes this key. */
const PARAM = "presenterMode";

/** The query string that opens a deck alongside its speaker notes. */
export const NOTES_SEARCH = `?${PARAM}=true`;

/**
 * Whether a URL's query string puts Spectacle into presenter mode.
 *
 * This mirrors Spectacle's own test instead of comparing against `"true"`.
 * Spectacle parses the query with `parseBooleans: true` and then tests the
 * result for plain truthiness, so `"true"` becomes boolean true, `"false"`
 * becomes boolean false, a valueless key becomes null, and every *other*
 * non-empty value survives as a truthy string.
 *
 * An exact `=== "true"` comparison is therefore strictly narrower than
 * Spectacle's, and the gap is not hypothetical: `?presenterMode=1` was
 * measured opening the presenter view with no exit button rendered — the one
 * state where that button is the whole point. Matching the semantics here is
 * what keeps the affordance and the mode in step.
 */
export function isPresenterMode(search: string): boolean {
  const value = new URLSearchParams(search).get(PARAM);
  return value !== null && value !== "" && value !== "false";
}

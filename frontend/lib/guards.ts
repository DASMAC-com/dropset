// Tiny type-guard utilities used across the UI. Keep this module
// dependency-free.

export const isFiniteNumber = (v: unknown): v is number =>
  typeof v === "number" && Number.isFinite(v);

// Pull a string message off an unknown thrown value without TypeScript noise.
// Replaces the `e instanceof Error ? e.message : String(e)` idiom that was
// open-coded in every catch.
export const getErrorMessage = (e: unknown): string =>
  e instanceof Error ? e.message : String(e);

// Tiny type-guard utilities used across the UI. Keep this module
// dependency-free.

export const isFiniteNumber = (v: unknown): v is number =>
  typeof v === "number" && Number.isFinite(v);

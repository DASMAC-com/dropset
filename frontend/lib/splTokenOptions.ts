// Options for `useSplToken` that let it auto-detect the SPL Token program
// (legacy vs Token-2022) from the mint's owner account. Without this the
// hook defaults to the legacy program, derives the wrong ATA address for
// Token-2022 mints (e.g. ZARP), and reports an empty balance.
//
// Hoisted to module scope so the options reference is stable across
// renders — the hook's internal `useMemo` keys on identity.
export const AUTO_DETECT_TOKEN_PROGRAM = {
  config: { tokenProgram: "auto" as const },
};

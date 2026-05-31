// Shared flag glyphs for the data tables. The Twemoji flag SVGs are already
// rounded rectangles, so we render them whole (no circular clip — that left a
// stray border on square flags like CH). Used singly on /currencies rows and
// as a pair on /vaults.

// A single full flag SVG.
export function Flag({ url, size }: { url: string; size: number }) {
  return (
    // biome-ignore lint/performance/noImgElement: tiny static SVG, no optimization needed
    <img src={url} alt="" aria-hidden width={size} height={size} />
  );
}

// Two flags side by side (base / quote of an FX pair).
export function FlagPair({
  base,
  quote,
  size,
}: {
  base: string;
  quote: string;
  size: number;
}) {
  return (
    <span className="flex shrink-0 items-center gap-1">
      <Flag url={base} size={size} />
      <Flag url={quote} size={size} />
    </span>
  );
}

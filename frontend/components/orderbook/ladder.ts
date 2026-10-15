import { type BookLevel, decodePrice } from "@dropset/sdk";
import { humanPrice } from "./format";

// Max rows rendered per side — a cap, not a pad. A vault rests up to
// N_LEVELS = 8 own levels per side, and as many again once its opposite side
// flips (a filled bid's proceeds rest as an ask, and the mirror), so one
// vault alone can put 2 × N_LEVELS prices on a side. Only the resting levels
// are rendered (no spacer padding), so the panel shrinks to the book; the
// empty→flashed moment reads as levels appearing in place.
export const MAX_ROWS = 16;

// One rendered ladder row: the raw on-chain price it was built from, that
// price in human units, the depth resting at it, and the cumulative size from
// the spread out to this row (the "Total" column).
//
// `bits` is carried so rows can be keyed by the price the chain actually
// quoted rather than by its rendered float — see the slot construction in
// ./OrderBook.
export type Row = {
  bits: BookLevel["price"];
  price: number;
  size: bigint;
  total: bigint;
};

// Best-first levels → rows with a running cumulative total (from the spread
// outward). Drops anything that doesn't decode to a real, positive price.
//
// Prices are scaled to human quote-per-base with the pair's decimals — see
// `humanPrice`. The sentinel guard still runs on the decoded float, since
// ZERO / INFINITY are properties of the `Price` itself and survive any scaling.
//
// Levels quoting the *same* on-chain price fold into one row, summing their
// depth. Two vaults may quote a price identically, and a flipped counter can
// land on a price an own level already quotes; the engine reports each
// separately, and its price-time sort puts them adjacent. A flipped side is
// otherwise an ordinary level — nobody outside the vault sees the bin — so
// this is the only place the view has to know flips exist. This mirrors the
// TUI ladder's merge, which likewise compares raw `Price` equality rather than
// a float tolerance, so the two books render the same ladder for the same
// market.
export function levelsToRows(
  levels: readonly BookLevel[],
  baseDecimals: number,
  quoteDecimals: number,
): Row[] {
  const rows: Row[] = [];
  let acc = 0n;
  for (const l of levels) {
    const decoded = decodePrice(l.price);
    if (!Number.isFinite(decoded) || decoded <= 0) continue;
    const last = rows.at(-1);
    if (last && last.bits === l.price) {
      acc += l.size;
      last.size += l.size;
      last.total = acc;
      continue;
    }
    if (rows.length >= MAX_ROWS) break;
    acc += l.size;
    rows.push({
      bits: l.price,
      price: humanPrice(l.price, baseDecimals, quoteDecimals),
      size: l.size,
      total: acc,
    });
  }
  return rows;
}

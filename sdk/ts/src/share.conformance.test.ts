/**
 * Verify the TS share / NAV / PnL fork against the shared share vectors —
 * the same `sdk/conformance/share_vectors.json` the Rust side checks
 * (sdk/math-core/tests/share_conformance.rs). This pins the hand-ported
 * kernels in `share.ts` to the engine math encoded in the generator
 * (sdk/math-core/examples/gen_share.rs).
 *
 * Integer fields are JSON strings (consensus values exceed JS's 2^53
 * safe-integer range), parsed back with `BigInt`; `*_bits` are raw u32
 * Price encodings (plain numbers).
 *
 * Run: `pnpm --filter @dropset/sdk test`.
 */

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import {
  BasketError,
  computeProRataSlice,
  CrystallizeError,
  crystallizePnl,
  isqrtU128,
  mergeEntryBasis,
  realizePerfFee,
  singleLegBasket,
} from './share';

type IsqrtCase = { n: string; expected: string };
type SlbOk = { shares_out: string; base_in_final: string; quote_in_final: string };
type SlbCase = {
  total_shares: string;
  base_atoms: string;
  quote_atoms: string;
  base_in: string;
  quote_in: string;
  max_base_in: string;
  max_quote_in: string;
  ok?: SlbOk;
  err?: string;
};
type ProRataCase = {
  shares_in: string;
  total_shares: string;
  base_atoms: string;
  quote_atoms: string;
  slice_base: string;
  slice_quote: string;
};
type RealizeCase = {
  base_atoms: string;
  quote_atoms: string;
  total_shares: string;
  leader_shares: string;
  hwm: string;
  perf_fee_rate: number;
  shares_minted: string;
  hwm_after: string;
  total_shares_after: string;
  leader_shares_after: string;
};
type CrystallizeOk = {
  realized_fx: string;
  realized_yield: string;
  realized_pnl: string;
  shares_after: string;
  net_deposits_after: string;
  pnl_delta: string;
};
type CrystallizeCase = {
  shares_in: string;
  shares: string;
  net_deposits: string;
  slice_base: string;
  slice_quote: string;
  entry_ref_bits: number;
  ref_now_bits: number;
  realized_fx: string;
  realized_yield: string;
  realized_pnl: string;
  ok?: CrystallizeOk;
  err?: string;
};
type MergeCase = {
  prior_shares: string;
  shares_out: string;
  entry_vps_prev: string;
  vps_after: string;
  entry_ref_prev_bits: number;
  ref_now_bits: number;
  entry_vps_new: string;
  entry_ref_new_bits: number;
};

const vectors = JSON.parse(
  readFileSync(new URL('../../conformance/share_vectors.json', import.meta.url), 'utf8'),
) as {
  isqrt: IsqrtCase[];
  single_leg_basket: SlbCase[];
  pro_rata_slice: ProRataCase[];
  realize_perf_fee: RealizeCase[];
  crystallize_pnl: CrystallizeCase[];
  merge_entry_basis: MergeCase[];
};

test('isqrt vectors match', () => {
  for (const c of vectors.isqrt) {
    assert.equal(isqrtU128(BigInt(c.n)), BigInt(c.expected), `isqrt(${c.n})`);
  }
});

test('single_leg_basket vectors match', () => {
  for (const c of vectors.single_leg_basket) {
    const call = () =>
      singleLegBasket(
        BigInt(c.total_shares),
        BigInt(c.base_atoms),
        BigInt(c.quote_atoms),
        BigInt(c.base_in),
        BigInt(c.quote_in),
        BigInt(c.max_base_in),
        BigInt(c.max_quote_in),
      );
    if (c.err !== undefined) {
      assert.throws(call, (e) => e instanceof BasketError && e.kind === c.err, `expected ${c.err}`);
    } else {
      const r = call();
      const ok = c.ok as SlbOk;
      assert.equal(r.sharesOut, BigInt(ok.shares_out), 'sharesOut');
      assert.equal(r.baseInFinal, BigInt(ok.base_in_final), 'baseInFinal');
      assert.equal(r.quoteInFinal, BigInt(ok.quote_in_final), 'quoteInFinal');
    }
  }
});

test('pro_rata_slice vectors match', () => {
  for (const c of vectors.pro_rata_slice) {
    const [sliceBase, sliceQuote] = computeProRataSlice(
      BigInt(c.shares_in),
      BigInt(c.total_shares),
      BigInt(c.base_atoms),
      BigInt(c.quote_atoms),
    );
    assert.equal(sliceBase, BigInt(c.slice_base), 'slice_base');
    assert.equal(sliceQuote, BigInt(c.slice_quote), 'slice_quote');
  }
});

test('realize_perf_fee vectors match', () => {
  for (const c of vectors.realize_perf_fee) {
    const r = realizePerfFee(
      BigInt(c.base_atoms),
      BigInt(c.quote_atoms),
      BigInt(c.total_shares),
      BigInt(c.leader_shares),
      BigInt(c.hwm),
      BigInt(c.perf_fee_rate),
    );
    assert.equal(r.sharesMinted, BigInt(c.shares_minted), 'sharesMinted');
    assert.equal(r.hwmAfter, BigInt(c.hwm_after), 'hwmAfter');
    assert.equal(r.totalSharesAfter, BigInt(c.total_shares_after), 'totalSharesAfter');
    assert.equal(r.leaderSharesAfter, BigInt(c.leader_shares_after), 'leaderSharesAfter');
  }
});

test('crystallize_pnl vectors match', () => {
  for (const c of vectors.crystallize_pnl) {
    const call = () =>
      crystallizePnl(
        BigInt(c.shares_in),
        BigInt(c.shares),
        BigInt(c.net_deposits),
        BigInt(c.slice_base),
        BigInt(c.slice_quote),
        c.entry_ref_bits,
        c.ref_now_bits,
        BigInt(c.realized_fx),
        BigInt(c.realized_yield),
        BigInt(c.realized_pnl),
      );
    if (c.err !== undefined) {
      assert.throws(
        call,
        (e) => e instanceof CrystallizeError && e.kind === c.err,
        `expected ${c.err}`,
      );
    } else {
      const r = call();
      const ok = c.ok as CrystallizeOk;
      assert.equal(r.realizedFx, BigInt(ok.realized_fx), 'realizedFx');
      assert.equal(r.realizedYield, BigInt(ok.realized_yield), 'realizedYield');
      assert.equal(r.realizedPnl, BigInt(ok.realized_pnl), 'realizedPnl');
      assert.equal(r.sharesAfter, BigInt(ok.shares_after), 'sharesAfter');
      assert.equal(r.netDepositsAfter, BigInt(ok.net_deposits_after), 'netDepositsAfter');
      assert.equal(r.pnlDelta, BigInt(ok.pnl_delta), 'pnlDelta');
    }
  }
});

test('merge_entry_basis vectors match', () => {
  for (const c of vectors.merge_entry_basis) {
    const [entryVpsNew, entryRefNew] = mergeEntryBasis(
      BigInt(c.prior_shares),
      BigInt(c.shares_out),
      BigInt(c.entry_vps_prev),
      BigInt(c.vps_after),
      c.entry_ref_prev_bits,
      c.ref_now_bits,
    );
    assert.equal(entryVpsNew, BigInt(c.entry_vps_new), 'entry_vps_new');
    assert.equal(entryRefNew >>> 0, c.entry_ref_new_bits, 'entry_ref_new_bits');
  }
});

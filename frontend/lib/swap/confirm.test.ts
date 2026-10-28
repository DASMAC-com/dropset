import { describe, expect, it } from "vitest";

import { waitForSwapConfirmation } from "./confirm";
import type { SwapError } from "./types";

const SIGNATURE = "sig" as never;

type Status = {
  err: unknown;
  confirmationStatus: "processed" | "confirmed" | "finalized";
} | null;

// An rpc that answers successive recent-status polls from `statuses` (the
// last entry repeating), every block-height read with `height`, and a
// full-history lookup with `history`.
const fakeRpc = ({
  statuses,
  height = 0n,
  history = null,
}: {
  statuses: Status[];
  height?: bigint;
  history?: Status;
}) => {
  let i = 0;
  return {
    getSignatureStatuses: (
      _: unknown,
      opts?: { searchTransactionHistory?: boolean },
    ) => ({
      send: async () => {
        if (opts?.searchTransactionHistory) return { value: [history] };
        const s = statuses[Math.min(i, statuses.length - 1)] ?? null;
        i++;
        return { value: [s] };
      },
    }),
    getBlockHeight: () => ({ send: async () => height }),
  } as never;
};

const FAST = { pollIntervalMs: 0, timeoutMs: 50 };
const ok = (cs: "processed" | "confirmed" | "finalized"): Status => ({
  err: null,
  confirmationStatus: cs,
});

const rejection = (p: Promise<void>) =>
  p.then(
    () => null,
    (e: unknown) => e as SwapError,
  );

describe("waitForSwapConfirmation", () => {
  it("resolves once the signature is finalized", async () => {
    const rpc = fakeRpc({ statuses: [null, ok("processed"), ok("finalized")] });
    await expect(
      waitForSwapConfirmation(rpc, SIGNATURE, {
        lastValidBlockHeight: 100n,
        ...FAST,
      }),
    ).resolves.toBeUndefined();
  });

  it("keeps waiting on an unseen signature while the blockhash is valid", async () => {
    // Many null polls, but the chain has not passed the expiry height: this
    // must not read as dropped, only as the timeout's unknown outcome.
    const rpc = fakeRpc({ statuses: [null], height: 100n });
    const e = await rejection(
      waitForSwapConfirmation(rpc, SIGNATURE, {
        lastValidBlockHeight: 100n,
        ...FAST,
      }),
    );
    expect(e?.kind).toBe("unconfirmed");
    expect(e?.signature).toBe(SIGNATURE);
  });

  it("declares a dropped transaction only past its expiry height", async () => {
    const rpc = fakeRpc({ statuses: [null], height: 101n });
    const e = await rejection(
      waitForSwapConfirmation(rpc, SIGNATURE, {
        lastValidBlockHeight: 100n,
        noun: "deposit",
        ...FAST,
      }),
    );
    expect(e?.kind).toBe("wallet");
    expect(e?.message).toMatch(/deposit expired/);
  });

  it("finds a late landing in full history before calling it dropped", async () => {
    const rpc = fakeRpc({
      statuses: [null],
      height: 101n,
      history: ok("finalized"),
    });
    await expect(
      waitForSwapConfirmation(rpc, SIGNATURE, {
        lastValidBlockHeight: 100n,
        ...FAST,
      }),
    ).resolves.toBeUndefined();
  });

  it("never declares dropped without an expiry height", async () => {
    const rpc = fakeRpc({ statuses: [null], height: 10_000n });
    const e = await rejection(
      waitForSwapConfirmation(rpc, SIGNATURE, {
        lastValidBlockHeight: null,
        ...FAST,
      }),
    );
    expect(e?.kind).toBe("unconfirmed");
  });

  it("treats a confirmed signature as landed when finalization times out", async () => {
    // A later null (another provider, behind) must not undo the landing.
    const rpc = fakeRpc({ statuses: [ok("confirmed"), null], height: 101n });
    await expect(
      waitForSwapConfirmation(rpc, SIGNATURE, {
        lastValidBlockHeight: 100n,
        ...FAST,
      }),
    ).resolves.toBeUndefined();
  });

  it("reports a revert with the signature attached", async () => {
    const rpc = fakeRpc({
      statuses: [
        {
          err: { InstructionError: [0n, { Custom: 6005n }] },
          confirmationStatus: "confirmed",
        },
      ],
    });
    const e = await rejection(
      waitForSwapConfirmation(rpc, SIGNATURE, {
        lastValidBlockHeight: 100n,
        ...FAST,
      }),
    );
    expect(e?.kind).toBe("wallet");
    expect(e?.message).toMatch(/reverted on-chain.*6005/);
    expect(e?.signature).toBe(SIGNATURE);
  });
});

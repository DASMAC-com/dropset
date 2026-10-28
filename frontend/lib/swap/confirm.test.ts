import { describe, expect, it } from "vitest";

import { waitForSwapConfirmation } from "./confirm";
import type { Lifetime, SwapError } from "./types";

const SIGNATURE = "sig" as never;
const LIFETIME = {
  blockhash: "hash",
  lastValidBlockHeight: 100n,
} as unknown as Lifetime;

type Status = {
  err: unknown;
  confirmationStatus: "processed" | "confirmed" | "finalized";
} | null;

// A scripted rpc. Recent-status polls answer from `statuses` (the last entry
// repeating; a `"throw"` entry rejects that poll). The finalized epoch info
// reports `height` at slot 500; the blockhash reads `blockhashValid`; the
// full-history lookup answers `history` from a node at `historySlot`.
const fakeRpc = ({
  statuses,
  height = 0n,
  blockhashValid = false,
  history = null,
  historySlot = 500n,
}: {
  statuses: (Status | "throw")[];
  height?: bigint;
  blockhashValid?: boolean;
  history?: Status;
  historySlot?: bigint;
}) => {
  let i = 0;
  return {
    getSignatureStatuses: (
      _: unknown,
      opts?: { searchTransactionHistory?: boolean },
    ) => ({
      send: async () => {
        if (opts?.searchTransactionHistory) {
          return { context: { slot: historySlot }, value: [history] };
        }
        const s = statuses[Math.min(i, statuses.length - 1)] ?? null;
        i++;
        if (s === "throw") throw new Error("429 Too Many Requests");
        return { context: { slot: 500n }, value: [s] };
      },
    }),
    getEpochInfo: () => ({
      send: async () => ({ blockHeight: height, absoluteSlot: 500n }),
    }),
    isBlockhashValid: () => ({
      send: async () => ({ context: { slot: 500n }, value: blockhashValid }),
    }),
  } as never;
};

const FAST = { lifetime: LIFETIME, pollIntervalMs: 0, timeoutMs: 50 };
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
      waitForSwapConfirmation(rpc, SIGNATURE, FAST),
    ).resolves.toBeUndefined();
  });

  it("keeps waiting on an unseen signature while the blockhash is valid", async () => {
    // Many null polls, but the chain has not passed the expiry height: this
    // must not read as dropped, only as the timeout's unknown outcome.
    const rpc = fakeRpc({ statuses: [null], height: 100n });
    const e = await rejection(waitForSwapConfirmation(rpc, SIGNATURE, FAST));
    expect(e?.kind).toBe("unconfirmed");
    expect(e?.signature).toBe(SIGNATURE);
  });

  it("declares a dropped transaction only past its expiry height", async () => {
    const rpc = fakeRpc({ statuses: [null], height: 101n });
    const e = await rejection(
      waitForSwapConfirmation(rpc, SIGNATURE, { ...FAST, noun: "deposit" }),
    );
    expect(e?.kind).toBe("wallet");
    expect(e?.message).toMatch(/deposit expired/);
  });

  it("does not trust the expiry height while the blockhash is still valid", async () => {
    // A too-low height from the builder: the chain says the blockhash is
    // still live, so the transaction can still land.
    const rpc = fakeRpc({
      statuses: [null],
      height: 101n,
      blockhashValid: true,
    });
    const e = await rejection(waitForSwapConfirmation(rpc, SIGNATURE, FAST));
    expect(e?.kind).toBe("unconfirmed");
  });

  it("ignores a history lookup from a node behind the expiry slot", async () => {
    // Failover can send the history lookup to a lagging provider that has not
    // yet seen the block the transaction landed in.
    const rpc = fakeRpc({ statuses: [null], height: 101n, historySlot: 499n });
    const e = await rejection(waitForSwapConfirmation(rpc, SIGNATURE, FAST));
    expect(e?.kind).toBe("unconfirmed");
  });

  it("finds a late landing in full history before calling it dropped", async () => {
    const rpc = fakeRpc({
      statuses: [null],
      height: 101n,
      history: ok("finalized"),
    });
    await expect(
      waitForSwapConfirmation(rpc, SIGNATURE, FAST),
    ).resolves.toBeUndefined();
  });

  it("never declares dropped without a lifetime", async () => {
    const rpc = fakeRpc({ statuses: [null], height: 10_000n });
    const e = await rejection(
      waitForSwapConfirmation(rpc, SIGNATURE, { ...FAST, lifetime: null }),
    );
    expect(e?.kind).toBe("unconfirmed");
  });

  it("skips a failed RPC read rather than reporting a failure", async () => {
    const rpc = fakeRpc({ statuses: ["throw", ok("finalized")] });
    await expect(
      waitForSwapConfirmation(rpc, SIGNATURE, FAST),
    ).resolves.toBeUndefined();
  });

  it("times out as unconfirmed when every read fails", async () => {
    const rpc = fakeRpc({ statuses: ["throw"] });
    const e = await rejection(waitForSwapConfirmation(rpc, SIGNATURE, FAST));
    expect(e?.kind).toBe("unconfirmed");
  });

  it("treats a confirmed signature as landed when finalization times out", async () => {
    // A later null or failed read (another provider) must not undo it.
    const rpc = fakeRpc({
      statuses: [ok("confirmed"), null, "throw"],
      height: 101n,
    });
    await expect(
      waitForSwapConfirmation(rpc, SIGNATURE, FAST),
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
    const e = await rejection(waitForSwapConfirmation(rpc, SIGNATURE, FAST));
    expect(e?.kind).toBe("wallet");
    expect(e?.message).toMatch(/reverted on-chain.*6005/);
    expect(e?.signature).toBe(SIGNATURE);
  });
});

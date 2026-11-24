import { ExternalLink } from "@/components/icons";
import { explorerTxUrl } from "@/lib/explorer";
import { groupThousands } from "@/lib/format/balance";
import type { UseVaultTx } from "@/lib/hooks/useVaultTx";

// A live vault transaction's progress, under its form's button.
export function TxStatusLine({ tx }: { tx: UseVaultTx }) {
  const text =
    tx.status === "signing"
      ? "Approve the transaction in your wallet…"
      : tx.status === "confirming"
        ? "Confirming onchain…"
        : tx.status === "success"
          ? tx.sharesAfter === null
            ? "Confirmed."
            : `Confirmed. You now hold ${groupThousands(tx.sharesAfter.toString())} shares.`
          : tx.status === "error" || tx.status === "unconfirmed"
            ? tx.error
            : null;
  if (!text) return null;
  const tone =
    tx.status === "error"
      ? "text-red-400"
      : tx.status === "success"
        ? "text-emerald-400"
        : "text-muted-fg";
  return (
    <p role="status" className={`text-center text-xs ${tone}`}>
      {text}
      {tx.signature && (
        <>
          {" "}
          <a
            href={explorerTxUrl(tx.signature)}
            target="_blank"
            rel="noreferrer"
            className="inline-flex items-center gap-0.5 underline hover:text-foreground"
          >
            View transaction <ExternalLink size={10} />
          </a>
        </>
      )}
    </p>
  );
}

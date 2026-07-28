// Explorer URL builders. On mainnet these point at Solscan: token mints get
// the richer `/token/` view (charts, holders, market info), wallet addresses use
// `/account/`, and signatures use `/tx/`.
//
// On localnet Solscan is useless — it can't see a loopback validator — so the
// links point at the local Solana Explorer container instead (infra/localnet,
// host port 3100), with the validator passed through the custom-cluster query
// params. The hosted explorer.solana.com is deliberately *not* the localnet
// fallback: a public HTTPS origin is blocked from fetching loopback in Brave
// and Safari, so it just stalls on "loading". `tui/src/explorer.rs` builds the
// same URLs for the TUI's links and documents that block at length.
import { IS_LOCALNET, PUBLIC_RPC_URL } from "./env";

const SOLSCAN = "https://solscan.io";

// What to call the explorer in link tooltips and labels. The URLs below are
// cluster-conditional, so the copy has to be too — a "View on Solscan" tooltip
// over a localhost link is just wrong.
export const EXPLORER_NAME = IS_LOCALNET ? "the local explorer" : "Solscan";

// Host port the explorer container publishes (compose maps `3100:3000`, which
// leaves localhost:3000 to the frontend's own `next dev`).
const LOCAL_EXPLORER = "http://localhost:3100";

// The custom-cluster suffix that points the local explorer at our validator.
const customCluster = () =>
  `?cluster=custom&customUrl=${encodeURIComponent(PUBLIC_RPC_URL)}`;

export const explorerAddressUrl = (address: string) =>
  IS_LOCALNET
    ? `${LOCAL_EXPLORER}/address/${address}${customCluster()}`
    : `${SOLSCAN}/account/${address}`;

// The local explorer has no token view, so a mint falls back to its address
// page there rather than linking somewhere that 404s.
export const explorerTokenUrl = (mint: string) =>
  IS_LOCALNET
    ? `${LOCAL_EXPLORER}/address/${mint}${customCluster()}`
    : `${SOLSCAN}/token/${mint}`;

export const explorerTxUrl = (signature: string) =>
  IS_LOCALNET
    ? `${LOCAL_EXPLORER}/tx/${signature}${customCluster()}`
    : `${SOLSCAN}/tx/${signature}`;

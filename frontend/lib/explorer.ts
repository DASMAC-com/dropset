// Solscan URL builders. Token mints get the richer `/token/` view (charts,
// holders, market info), wallet addresses use `/account/`, and signatures
// use `/tx/`.
const SOLSCAN = "https://solscan.io";

export const explorerAddressUrl = (address: string) =>
  `${SOLSCAN}/account/${address}`;

export const explorerTokenUrl = (mint: string) => `${SOLSCAN}/token/${mint}`;

export const explorerTxUrl = (signature: string) =>
  `${SOLSCAN}/tx/${signature}`;

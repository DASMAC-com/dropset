// Codama codegen for the Dropset clients.
//
// Pipeline (interface.md § SDK, spine A): the checked-in `anchor-next`
// IDL -> a Codama tree -> TypeScript (`@solana/kit`) and Rust clients
// (instruction builders, account / event codecs, PDA helpers).
//
// Regenerate with `make sdk` (or `pnpm generate` here) after `make idl`.

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  arrayTypeNode,
  bottomUpTransformerVisitor,
  createFromRoot,
  definedTypeNode,
  fixedCountNode,
  numberTypeNode,
} from 'codama';
import { rootNodeFromAnchor } from '@codama/nodes-from-anchor';
import { renderVisitor as renderJavaScript } from '@codama/renderers-js';
import { renderVisitor as renderRust } from '@codama/renderers-rust';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, '..', '..');
const idlPath = join(repoRoot, 'sdk', 'idl', 'dropset.json');

const idl = JSON.parse(readFileSync(idlPath, 'utf8'));
const codama = createFromRoot(rootNodeFromAnchor(idl));

// The renderers emit codecs for `definedTypes` but not for `program.events`,
// so the 5 `#[event]` structs (Deposit/Withdraw/OpenVault/Realize/Fill)
// don't get codecs. interface.md §1/§2 puts event decoding in the SDK's
// remit (the indexer strips the `[tag][discriminator]` envelope, then the
// Codama struct codec decodes the borsh body). Inject each event's struct
// as a defined type — dropping the discriminator prefix, since the codec
// decodes the body only — so `getDepositEventDecoder` etc. are generated.
codama.update(
  bottomUpTransformerVisitor([
    {
      select: '[programNode]',
      transform: (program) => {
        const eventTypes = (program.events ?? []).map((e) => {
          const struct = e.data.kind === 'hiddenPrefixTypeNode' ? e.data.type : e.data;
          return definedTypeNode({ name: e.name, type: struct, docs: e.docs ?? [] });
        });
        return { ...program, definedTypes: [...program.definedTypes, ...eventTypes] };
      },
    },
  ]),
);

// `Price` is a u32 decimal-float comparison key, but it does not derive
// `IdlType`, so the program's IDL surfaces it as a *fieldless* struct.
// Left as-is, Codama would emit a zero-byte codec and silently corrupt
// every `Price`-bearing decode (FillEvent.fill_price, ReferencePrice).
// Remap the defined type to its true wire form — a bare `u32` — so the
// generated codecs read the right four bytes. The human-facing decimal
// codec (bits <-> value) is layered by hand in each client's `price`
// module, keyed off these raw bits.
codama.update(
  bottomUpTransformerVisitor([
    {
      select: '[definedTypeNode]price',
      transform: (node) => ({ ...node, type: numberTypeNode('u32') }),
    },
    {
      // `set_liquidity_profile`'s `profile_bytes: [u8; PROFILE_BYTES]`
      // surfaces as a zero-length array — anchor-next can't const-eval
      // `PROFILE_BYTES` (= size_of::<LiquidityProfile>() = 160). Restore
      // the real length so the generated arg is `[u8; 160]`, matching the
      // serialized LiquidityProfile the instruction expects.
      select: '[instructionArgumentNode]profileBytes',
      transform: (node) => ({
        ...node,
        type: arrayTypeNode(numberTypeNode('u8'), fixedCountNode(160)),
      }),
    },
  ]),
);

const tsOut = join(repoRoot, 'sdk', 'ts', 'src', 'generated');
const rustCrate = join(repoRoot, 'sdk', 'rs');
const rustOut = join(rustCrate, 'src', 'generated');

// Neither client is formatted here: Codama's bundled formatters don't
// match the repo's tools (its rustfmt diverges from `cargo fmt`). The
// `make sdk` target runs `cargo fmt` after this so the generated Rust
// lands in canonical `cargo fmt` form — clean under the rustfmt hook and
// reproducible by the SDK CI gate. TS is left raw: biome lints only
// frontend/, and raw output is still deterministic.
codama.accept(renderJavaScript(tsOut, { formatCode: false }));
codama.accept(
  renderRust(rustOut, { formatCode: false, crateFolder: rustCrate }),
);

console.log('Generated TypeScript client ->', tsOut);
console.log('Generated Rust client       ->', rustOut);

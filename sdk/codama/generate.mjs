// Codama codegen for the Dropset clients.
//
// Pipeline (interface.md § SDK, spine A): the checked-in `anchor-next`
// IDL -> a Codama tree -> TypeScript (`@solana/kit`) and Rust clients
// (instruction builders, account / event codecs, PDA helpers).
//
// Regenerate with `make sdk` (or `pnpm generate` here) after `make idl`.

import { readdirSync, readFileSync, writeFileSync } from 'node:fs';
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

const idl = inlineTypeAliases(JSON.parse(readFileSync(idlPath, 'utf8')));
const codama = createFromRoot(rootNodeFromAnchor(idl));

/**
 * Inline the IDL's type aliases before Codama reads it.
 *
 * anchor-next emits each Pod wrapper (`PodU64`, `PodU32`, ...) as a
 * `{ kind: "type", alias: <primitive> }` defined type — the IDL spec's
 * `IdlTypeDefTy::Type` — which `@codama/nodes-from-anchor` does not
 * recognize (it knows only the older `{ kind: "alias", value }`). The
 * wrappers are an on-chain alignment detail with the same bytes as their
 * primitive, so replace every reference with the aliased type and drop
 * the alias definitions: the clients keep plain `u64` / `bigint` fields.
 */
function inlineTypeAliases(idl) {
  const aliases = new Map(
    idl.types.filter((t) => t.type.kind === 'type').map((t) => [t.name, t.type.alias]),
  );
  const resolve = (node) => {
    if (Array.isArray(node)) return node.map(resolve);
    if (node === null || typeof node !== 'object') return node;
    const name = node.defined?.name;
    if (aliases.has(name)) return resolve(aliases.get(name));
    return Object.fromEntries(Object.entries(node).map(([k, v]) => [k, resolve(v)]));
  };
  return resolve({ ...idl, types: idl.types.filter((t) => !aliases.has(t.name)) });
}

/** Levels per side of a `LiquidityProfile` — the program's `N_LEVELS`. */
const N_LEVELS = 8;
/**
 * On-wire width of one `Level`: `price_offset` u32, `flip_spread_ppm` u32,
 * `size_bps` u16, and a per-domain expiry offset each
 * (`expiry_offset_secs` / `_slots`, u32). Every field is alignment-1, so
 * the struct is byte-packed.
 */
const LEVEL_BYTES = 4 + 4 + 2 + 4 + 4;
/** Serialized `LiquidityProfile`: both sides' level arrays, back to back. */
const PROFILE_BYTES = 2 * N_LEVELS * LEVEL_BYTES;

// The renderers emit codecs for `definedTypes` but not for `program.events`,
// so the 5 `#[event]` structs (Deposit/Withdraw/CreateVault/Realize/Fill)
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

// `Price` is a u32 decimal-float comparison key; the IDL surfaces it as a
// one-field tuple struct, which Codama would render as a tuple type.
// Remap the defined type to its bare wire form — a `u32` — because the
// human-facing decimal codec (bits <-> value) is layered by hand in each
// client's `price` module, keyed off these raw bits. (Older anchor-next
// revs emitted it fieldless, which also mis-sized every account holding a
// `Price` — `VaultDepositorHeader` read 148 bytes instead of 152.)
codama.update(
  bottomUpTransformerVisitor([
    {
      select: '[definedTypeNode]price',
      transform: (node) => ({ ...node, type: numberTypeNode('u32') }),
    },
  ]),
);

// `set_liquidity_profile`'s `profile_bytes: [u8; PROFILE_BYTES]` must match
// the serialized LiquidityProfile. The IDL carries the length (older
// anchor-next revs emitted 0 and this script restored it); fail the
// generation if it ever disagrees with the hand-derived width, since a
// mismatch is a silent wire-format break rather than a build error. The
// value has moved twice already (160 → 224, when per-level expiry gained
// its second domain; 224 → 288, when each level gained its flip spread).
const profileArg = idl.instructions
  .find((ix) => ix.name === 'set_liquidity_profile')
  .args.find((arg) => arg.name === 'profile_bytes');
if (profileArg.type.array[1] !== PROFILE_BYTES) {
  throw new Error(
    `profile_bytes is [u8; ${profileArg.type.array[1]}] in the IDL, expected ${PROFILE_BYTES}`,
  );
}
// Codama renders an IDL `[u8; N]` as fixed bytes (`ReadonlyUint8Array` in
// TS); the clients take the profile as a u8 array (`Array<number>`), so
// keep it an array node of the same width.
codama.update(
  bottomUpTransformerVisitor([
    {
      select: '[instructionArgumentNode]profileBytes',
      transform: (node) => ({
        ...node,
        type: arrayTypeNode(numberTypeNode('u8'), fixedCountNode(PROFILE_BYTES)),
      }),
    },
  ]),
);

// An async builder resolves a defaulted PDA account (registry, event
// authority, market, vault depositor) by calling the linked `find*Pda`
// helper with its seeds and *no* program address, so the derivation falls
// back to the IDL's localnet id even when the builder was handed the mainnet
// one. Inline each link to this program's own PDA node instead: the
// renderer then derives it with `getProgramDerivedAddress({ programAddress,
// … })`, against the program address the builder itself resolved.
const rootProgram = codama.getRoot().program;
const programPdas = new Map(rootProgram.pdas.map((pda) => [pda.name, pda]));
codama.update(
  bottomUpTransformerVisitor([
    {
      select: '[pdaValueNode]',
      transform: (node) => {
        if (node.pda.kind !== 'pdaLinkNode') return node;
        const linked = node.pda.program?.name;
        if (linked && linked !== rootProgram.name) return node;
        const pda = programPdas.get(node.pda.name);
        if (!pda || pda.programId) return node;
        return { ...node, pda };
      },
    },
  ]),
);

/**
 * Make the program address a required input of the generated TS client.
 *
 * The program is deployed at a different address per cluster, but the
 * renderer makes `config.programAddress` optional everywhere and defaults
 * it to the IDL's localnet id, so a caller that forgets it gets a
 * localnet-addressed instruction or PDA on mainnet with nothing to say so.
 * renderers-js has no option to make the config required, and a
 * hand-written wrapper would have to restate each builder's generic
 * signature, so this rewrites the rendered text instead. It rewrites four
 * sites: builder configs, PDA-finder configs, the parsers' absent-optional
 * sentinel (the instruction's own program, not the localnet id), and
 * `isDropsetError`, which gains a `programAddress` parameter.
 *
 * Every rewrite is a match with the count the file calls for, and a miss
 * throws; a closing scan then refuses any localnet fallback left anywhere in
 * the tree. A renderer bump that changes the shape fails generation rather
 * than silently restoring the default.
 */
function requireTsProgramAddress(outDir) {
  // `from` is a string or a global RegExp; `count` is the expected number of
  // matches.
  const rewrite = (path, rules) => {
    let text = readFileSync(path, 'utf8');
    for (const { from, to, count } of rules) {
      const hits =
        typeof from === 'string'
          ? text.split(from).length - 1
          : (text.match(from) ?? []).length;
      if (hits !== count) {
        throw new Error(`${path}: ${hits} match(es) of ${from}, expected ${count}`);
      }
      text =
        typeof from === 'string' ? text.split(from).join(to) : text.replace(from, to);
    }
    writeFileSync(path, text);
  };
  const sources = (dir) =>
    readdirSync(join(outDir, dir))
      .filter((f) => f.endsWith('.ts') && f !== 'index.ts')
      .map((f) => join(outDir, dir, f));

  for (const path of sources('instructions')) {
    // One sync builder, plus an async one when the instruction has PDA or
    // other async defaults; the parser has the sentinel only when the
    // instruction has an optional account.
    const text = readFileSync(path, 'utf8');
    const builders = (text.match(/^export (async )?function get\w+Instruction(Async)?</gm) ?? [])
      .length;
    const optional = text.includes('const getNextOptionalAccount = ') ? 1 : 0;
    rewrite(path, [
      {
        from: 'config?: { programAddress?: TProgramAddress } )',
        to: 'config: { programAddress: TProgramAddress } )',
        count: builders,
      },
      {
        from: 'const programAddress = config?.programAddress ?? DROPSET_PROGRAM_ADDRESS;',
        to: 'const programAddress = config.programAddress;',
        count: builders,
      },
      {
        from: 'accountMeta.address === DROPSET_PROGRAM_ADDRESS ? undefined',
        to: 'accountMeta.address === instruction.programAddress ? undefined',
        count: optional,
      },
    ]);
  }

  for (const path of sources('pdas')) {
    rewrite(path, [
      {
        from: /const \{ programAddress = '\w+' as Address<'\w+'> \} = config;/g,
        to: 'const { programAddress } = config;',
        count: 1,
      },
      {
        from: 'config: { programAddress?: Address | undefined } = {}): Promise<',
        to: 'config: { programAddress: Address }): Promise<',
        count: 1,
      },
    ]);
  }

  rewrite(join(outDir, 'errors', 'dropset.ts'), [
    {
      from: "import { DROPSET_PROGRAM_ADDRESS } from '../programs';\n",
      to: '',
      count: 1,
    },
    {
      from: '    transactionMessage: { instructions: Record<number, { programAddress: Address }> },\n',
      to: '    transactionMessage: { instructions: Record<number, { programAddress: Address }> },\n    programAddress: Address,\n',
      count: 1,
    },
    {
      from: 'transactionMessage, DROPSET_PROGRAM_ADDRESS, code)',
      to: 'transactionMessage, programAddress, code)',
      count: 1,
    },
  ]);

  // Whatever the renderer emitted, the localnet id may survive only as the
  // constant's own definition, its imports, and type-level defaults. Any
  // other use, an optional program address, or a no-config PDA call is a
  // fallback the rules above missed.
  const fallback =
    /(?<!typeof |import \{ |export const )DROPSET_PROGRAM_ADDRESS|programAddress\?:|await find\w+Pda\(/;
  for (const dir of ['instructions', 'pdas', 'errors', 'accounts', 'programs']) {
    for (const path of sources(dir)) {
      const hit = readFileSync(path, 'utf8').match(fallback);
      if (hit) throw new Error(`${path}: localnet fallback survives: ${hit[0]}`);
    }
  }
}

const tsOut = join(repoRoot, 'sdk', 'ts', 'src', 'generated');
const rustCrate = join(repoRoot, 'sdk', 'rs');
const rustOut = join(rustCrate, 'src', 'generated');

// Neither client is formatted here: Codama's bundled formatters don't
// match the repo's tools (its rustfmt diverges from `cargo fmt`). The
// `make sdk` target runs `cargo fmt` after this so the generated Rust
// lands in canonical `cargo fmt` form — clean under the rustfmt hook and
// reproducible by the SDK CI gate. TS is left raw: biome lints only
// frontend/, and raw output is still deterministic.
// The renderer writes asynchronously; await it so the rewrite below finds
// the files.
await codama.accept(renderJavaScript(tsOut, { formatCode: false }));
requireTsProgramAddress(tsOut);
codama.accept(
  renderRust(rustOut, { formatCode: false, crateFolder: rustCrate }),
);

console.log('Generated TypeScript client ->', tsOut);
console.log('Generated Rust client       ->', rustOut);

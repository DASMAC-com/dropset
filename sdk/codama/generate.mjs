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

const idl = JSON.parse(readFileSync(idlPath, 'utf8'));
const codama = createFromRoot(rootNodeFromAnchor(idl));

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
      // `PROFILE_BYTES` (= size_of::<LiquidityProfile>()). Restore the
      // real length so the generated arg matches the serialized
      // LiquidityProfile the instruction expects.
      //
      // Derived here rather than written as a literal, because the value
      // has moved twice already (160 → 224, when per-level expiry gained
      // its second domain; 224 → 288, when each level gained its flip
      // spread) and a stale literal here is a silent wire-format
      // mismatch rather than a build break: 2 sides × 8 levels × an
      // 18-byte Level (price_offset u32, flip_spread_ppm u32, size_bps
      // u16, expiry_offset_secs u32, expiry_offset_slots u32).
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
// Awaited: the renderer may write asynchronously, and the rewrite below must
// see its output, not the previous run's.
await codama.accept(
  renderRust(rustOut, { formatCode: false, crateFolder: rustCrate }),
);

// The Rust renderer hard-codes the program id as `crate::DROPSET_ID`, the
// localnet id, and has no option to take it at runtime. That id's secret is
// committed, so a client that forgets to pick its cluster would sign for a
// program anyone can deploy. Rewrite each client-side builder to take a
// `crate::Deployment` as its first argument and aim at its id, so forgetting
// is a compile error. The CPI half below `<Name>CpiAccounts` already carries
// the program as an account, so it aims at that instead — which makes the
// invoked program caller-supplied: an on-chain caller must constrain that
// account to the Dropset id for its cluster before invoking.
const deploymentParam = 'deployment: crate::Deployment';
const instructionsOut = join(rustOut, 'instructions');
for (const file of readdirSync(instructionsOut)) {
  if (!file.endsWith('.rs') || file === 'mod.rs') continue;
  const path = join(instructionsOut, file);
  const source = readFileSync(path, 'utf8');
  const cpiAt = source.search(/pub struct \w+CpiAccounts\b/);
  if (cpiAt === -1) {
    throw new Error(`${file}: no CpiAccounts struct to split on`);
  }
  const client = source
    .slice(0, cpiAt)
    .replace(
      /pub fn instruction\(\s*&self/g,
      `pub fn instruction(&self, ${deploymentParam}`,
    )
    .replace(
      /pub fn instruction_with_remaining_accounts\(\s*&self\s*,/g,
      `pub fn instruction_with_remaining_accounts(&self, ${deploymentParam},`,
    )
    .replace(
      /\.instruction_with_remaining_accounts\(/g,
      '.instruction_with_remaining_accounts(deployment, ',
    )
    .replaceAll('crate::DROPSET_ID', 'deployment.program_id()');
  const cpi = source
    .slice(cpiAt)
    .replaceAll('crate::DROPSET_ID', '*self.__program.key');
  // An accounts struct's two builders plus the `<Name>Builder` one: fewer
  // means the renderer's output moved and the rewrite missed a path.
  const rewritten = client.split(deploymentParam).length - 1;
  if (rewritten !== 3) {
    throw new Error(`${file}: rewrote ${rewritten} builders, expected 3`);
  }
  // Any spelling of the localnet id left behind (a renderer that names it
  // another way) would be a builder that takes a deployment and ignores it.
  if (/DROPSET_ID/.test(client + cpi)) {
    throw new Error(`${file}: the localnet id survived the rewrite`);
  }
  writeFileSync(path, client + cpi);
}

console.log('Generated TypeScript client ->', tsOut);
console.log('Generated Rust client       ->', rustOut);

# SVM heap + Anchor event-cpi: deep-research synthesis

Source-grounded notes feeding the **Order matching** and **Events and
emission** sections of `docs/architecture.md` (lines ~1370–1545).
Every load-bearing claim cites a specific file path + line range in
either the local `~/repos/agave`, `~/repos/sbpf`, `~/repos/anchor`, or
`~/repos/pinocchio` checkouts, or in published crates.

Workflow stats: 7 research dimensions • 78 raw claims • 59 confirmed
• 19 refuted • 242 agent invocations.

## 1. SVM heap mechanics (what we're actually getting)

**Default and max heap.** Programs get 32 KiB heap by default; the
runtime accepts requests in [32 KiB, 256 KiB] in 1 KiB multiples.

- `pub const HEAP_LENGTH: usize = 32 * 1024;` at
  [`solana-program-entrypoint-3.1.1/src/lib.rs:37-39`](https://github.com/anza-xyz/solana-sdk/blob/program-entrypoint%40v3.1.1/program-entrypoint/src/lib.rs#L37-L39)
- `pub const MAX_HEAP_FRAME_BYTES: u32 = 256 * 1024;` at
  [`agave/program-runtime/src/execution_budget.rs:48`](https://github.com/anza-xyz/agave/blob/1ad187441b53d2ffb8f41a99e06f44ae27fda219/program-runtime/src/execution_budget.rs#L48)
- Sanitization:
  `(MIN_HEAP_FRAME_BYTES..=MAX_HEAP_FRAME_BYTES).contains(&bytes) && bytes.is_multiple_of(1024)`
  at [`agave/compute-budget-instruction/src/compute_budget_instruction_details.rs:192-194`](https://github.com/anza-xyz/agave/blob/1ad187441b53d2ffb8f41a99e06f44ae27fda219/compute-budget-instruction/src/compute_budget_instruction_details.rs#L192-L194)

**CU cost.** 8 CU per extra 32 KiB page. 32 KiB → 0 CU,
64 KiB → 8 CU, 256 KiB → 56 CU. Charged once up front when the VM is
created.

- `pub const DEFAULT_HEAP_COST: u64 = 8;` at
  [`agave/program-runtime/src/execution_budget.rs:41-43`](https://github.com/anza-xyz/agave/blob/1ad187441b53d2ffb8f41a99e06f44ae27fda219/program-runtime/src/execution_budget.rs#L41-L43)
- `calculate_heap_cost` at [`agave/program-runtime/src/vm.rs:34-45`](https://github.com/anza-xyz/agave/blob/1ad187441b53d2ffb8f41a99e06f44ae27fda219/program-runtime/src/vm.rs#L34-L45)
- Charge site: `invoke_context.consume_checked(calculate_heap_cost(...))`
  at [`agave/program-runtime/src/vm.rs:128-133`](https://github.com/anza-xyz/agave/blob/1ad187441b53d2ffb8f41a99e06f44ae27fda219/program-runtime/src/vm.rs#L128-L133)

**VM memory map.** Four 4 GiB virtual regions. The heap is region 3.

```
MM_BYTECODE_START = 1 << 32   // 0x1_0000_0000  program (RO)
MM_STACK_START    = 2 << 32   // 0x2_0000_0000  stack (RW)
MM_HEAP_START     = 3 << 32   // 0x3_0000_0000  heap  (RW)
MM_INPUT_START    = 4 << 32   // 0x4_0000_0000  accounts (RW)
```

- Constants at [`solana-sbpf-0.16.0/src/ebpf.rs:38-51`](https://github.com/anza-xyz/sbpf/blob/v0.16.0/src/ebpf.rs#L38-L51)
- Heap region wired writable:
  `MemoryRegion::new_writable(heap, MM_HEAP_START)` at
  [`agave/program-runtime/src/vm.rs:105`](https://github.com/anza-xyz/agave/blob/1ad187441b53d2ffb8f41a99e06f44ae27fda219/program-runtime/src/vm.rs#L105)
- OOB heap access labelled `"heap"`: [`memory_region.rs:219-224`](https://github.com/anza-xyz/sbpf/blob/239cb0bb771224bc49ca679c6a93ee7a876e8cbc/src/memory_region.rs#L219-L224)

**Default allocator: bump, no free.** Anchor v2 programs install
Pinocchio's `BumpAllocator` via `#[program]` expansion
([`anchor/lang-v2/derive/src/lib.rs:4656-4660`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang-v2/derive/src/lib.rs#L4656-L4660):
`pinocchio::default_allocator!()`). `dealloc` is a no-op:

```rust
// pinocchio/sdk/src/entrypoint/mod.rs:832-833
unsafe fn dealloc(&self, _: *mut u8, _: Layout) {}
```

The Pinocchio default writes the bump pointer into the first 8 bytes
of the heap region
([`pinocchio/sdk/src/entrypoint/mod.rs:796-806`](https://github.com/anza-xyz/pinocchio/blob/009301423f920fd105bd32a25560d127b6f0bf4f/sdk/src/entrypoint/mod.rs#L796-L806)), so usable heap =
`heap_size - sizeof(usize) - alignment_slack`.

Note: the *solana-program* (non-Pinocchio) default `BumpAllocator` is
hard-coded to `HEAP_LENGTH=32 KiB`
([`solana-program-entrypoint-3.1.1/src/lib.rs:219-230`](https://github.com/anza-xyz/solana-sdk/blob/program-entrypoint%40v3.1.1/program-entrypoint/src/lib.rs#L219-L230)); enlarging via
`RequestHeapFrame` requires a custom `#[global_allocator]` to actually
USE the extra bytes. Anchor v2 + Pinocchio sidesteps this —
Pinocchio's macro is parameterized on the runtime heap size.

**Legacy `sol_alloc_free_` is dead** for new deployments
([`agave/syscalls/src/lib.rs:476-481`](https://github.com/anza-xyz/agave/blob/1ad187441b53d2ffb8f41a99e06f44ae27fda219/syscalls/src/lib.rs#L476-L481), gated by
`disable_deploy_of_alloc_free_syscall`). Ignore.

## 2. Stack vs heap for the ephemeral book

**Stack constraints.** Default SBPF frame = 4096 B, max call
depth = 64, total stack = 256 KiB ([`sbpf/src/vm.rs:107-110`](https://github.com/anza-xyz/sbpf/blob/239cb0bb771224bc49ca679c6a93ee7a876e8cbc/src/vm.rs#L107-L110),
[`vm.rs:99-101`](https://github.com/anza-xyz/sbpf/blob/239cb0bb771224bc49ca679c6a93ee7a876e8cbc/src/vm.rs#L99-L101)). Any single allocation > ~3 KiB in a frame risks
overflow; recursion bounded at 64.

→ **The ephemeral book MUST be heap-allocated.** Even a
320-entry × 24 B book (7.7 KiB) blows the 4 KiB frame.

**Allocation idioms ranked.**

| idiom                          | when                                                                                          |
| ------------------------------ | --------------------------------------------------------------------------------------------- |
| `BinaryHeap::with_capacity(N)` | needs `pop_min` — this is the matching engine                                                 |
| `Box<[T; N]>`                  | fixed-size slab, no pop-min semantics; you'd reimplement sift-down                            |
| `Vec::with_capacity(N)`        | not enough by itself for matching                                                             |
| `Vec::new()` + push            | **forbidden** in matching loop — every realloc leaks the old buffer on the bump allocator     |

**Capacity math.** Spec entry is
`(price_key:u32, nonce:u64, size:u64, vault_idx:u16, level_idx:u8)`.
With natural u64 alignment that's ~24 B/entry. At
`MAX_VAULTS_PER_MARKET = 10` ([`programs/dropset/src/state.rs:26`](https://github.com/DASMAC-com/dropset/blob/ecfc46fd5e8c627292aefd627899cd05cb28df61/programs/dropset/src/state.rs#L26))
× 32 levels/side = 320 entries × 24 B = **7,680 B**. Fits the 32 KiB
default heap with > 3× headroom.

**When to `request_heap_frame`.** Only when
`vaults × levels × 24 B + other_alloc_pressure` approaches ~24 KiB
usable. For dropset's documented scale: not needed. If
`MAX_VAULTS_PER_MARKET` ever grows to hundreds, request 64 KiB
(cost: 8 CU).

**v2 import note.** `lang-v2` is `#![no_std]`
([`anchor/lang-v2/src/lib.rs:5-6`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang-v2/src/lib.rs#L5-L6)). Use `alloc::collections::BinaryHeap`,
not `std::collections::BinaryHeap`.

## 3. The matching engine on the heap (concrete recipe)

**The keyed-entry shape.** Translate prices to u32 at push time so
derived `Ord` does the work without a per-compare branch:

```rust
use alloc::collections::BinaryHeap;
use core::cmp::Reverse;

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
#[repr(C)]
struct HeapEntry {
    // Field order = lexicographic comparison order.
    key: u32,        // bid: Price::bid_key(p); ask: p.as_u32()
    nonce: u64,      // (stamp & !FLUSH_BIT) — masked at push
    size: u64,
    vault_idx: u16,
    level_idx: u8,
}

let cap = (header.vaults as usize) * MAX_LEVELS_PER_SIDE;
let mut heap: BinaryHeap<Reverse<HeapEntry>> = BinaryHeap::with_capacity(cap);
```

`Price::bid_key()` already exists at
[`programs/dropset/src/price.rs:228-234`](https://github.com/DASMAC-com/dropset/blob/ecfc46fd5e8c627292aefd627899cd05cb28df61/programs/dropset/src/price.rs#L228-L234). Asks use `as_u32()` directly.
Both sides feed the *same* min-heap shape.

**Sizing.** `with_capacity(header.vaults × MAX_LEVELS_PER_SIDE)`.
Single up-front allocation. **Never push past capacity** —
`BinaryHeap` (backed by `Vec`) doubles on overflow and the bump
allocator never reclaims the old buffer
([`pinocchio/sdk/src/entrypoint/mod.rs:832-833`](https://github.com/anza-xyz/pinocchio/blob/009301423f920fd105bd32a25560d127b6f0bf4f/sdk/src/entrypoint/mod.rs#L832-L833)).

**Tear-down.** Heap is dropped at instruction return when the VM
tears down the entire heap region. No explicit `dealloc` is run
(bump allocator's `dealloc` is a no-op anyway). Zero work; zero CU.

**Pitfalls.**

- **Grow-time realloc leak.** Pre-size, don't push-past-capacity.
  Each doubling permanently consumes `old + new` bytes for the rest
  of the instruction.
- **Key inversion mistake.** Don't put `Price` in the tuple and flip
  the comparator — translate to `u32` at push time. Lexicographic
  field order does the rest.
- **FLUSH_BIT masking.** Spec line 1417: `stamp & !FLUSH_BIT`. Mask
  at push time, not at compare time.
- **Stack pressure from event literal.** `emit_cpi!(EclobFill { .. })`
  puts the struct literal on the stack at the macro site
  ([`anchor/lang/attribute/event/src/lib.rs:166-195`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang/attribute/event/src/lib.rs#L166-L195)). Keep
  fill-event structs small (fixed-size primitives only).

## 4. Anchor `emit_cpi!` mechanics

**Macro expansion (v1; v2 is functionally identical).**

```rust
// anchor/lang/attribute/event/src/lib.rs:159-196
let disc = anchor_lang::event::EVENT_IX_TAG_LE;          // 8 bytes
let inner_data = anchor_lang::Event::data(&event_struct); // [8B event-disc | borsh body]
let ix_data: Vec<u8> = disc.into_iter().map(|b| *b)
                          .chain(inner_data.into_iter())
                          .collect();
let ix = Instruction::new_with_bytes(
    crate::ID,
    &ix_data,
    vec![AccountMeta::new_readonly(*authority_info.key, true)],
);
invoke_signed(&ix, &[authority_info],
              &[&[b"__event_authority", &[crate::EVENT_AUTHORITY_AND_BUMP.1]]])?;
```

v2 expansion at [`anchor/lang-v2/derive/src/lib.rs:5400-5415`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang-v2/derive/src/lib.rs#L5400-L5415) does the
same with `CpiContext::invoke`.

**On-the-wire data layout.**

```
[ 8 B EVENT_IX_TAG_LE = e4 45 a5 2e 51 cb 9a 1d ]   // sha256("anchor:event")[..8]
[ 8 B event-discriminator = sha256("event:<EventName>")[..8] ]
[ borsh-encoded event body                         ]
```

- `EVENT_IX_TAG = 0x1d9acb512ea545e4` at [`anchor/lang/src/event.rs:1-3`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang/src/event.rs#L1-L3)
- Event disc generator at
  [`anchor/lang/attribute/event/src/lib.rs:42-44`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang/attribute/event/src/lib.rs#L42-L44) and
  [`lang/syn/src/codegen/program/common.rs:11-16`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang/syn/src/codegen/program/common.rs#L11-L16)
- Per-event `Event::data` preallocates 256 B (was 1024 pre-0.30):
  [`anchor/lang/attribute/event/src/lib.rs:50-56`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang/attribute/event/src/lib.rs#L50-L56)

**`event_authority` PDA.** Seeds `[b"__event_authority"]`;
address+bump baked at compile time as
`crate::EVENT_AUTHORITY_AND_BUMP`.

- Seeds:
  [`anchor/lang/syn/src/parser/accounts/event_cpi.rs:13-18`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang/syn/src/parser/accounts/event_cpi.rs#L13-L18)
- Bake: [`anchor/lang/attribute/account/src/id.rs:43-53`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang/attribute/account/src/id.rs#L43-L53)

**Injected accounts (last two on the Accounts struct).**

```rust
// anchor/lang-v2/derive/src/lib.rs:5459-5470
#[account(seeds = [b"__event_authority"], bump)]
pub event_authority: UncheckedAccount,
pub program: UncheckedAccount,
```

Caller passes them as `(event_authority_pda, is_signer=false,
is_writable=false)` and `(program_id, false, false)`. The program
upgrades `event_authority` to a signer via `invoke_signed` (tests:
[`anchor/tests-v2/tests/event_cpi.rs:79-93`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/tests-v2/tests/event_cpi.rs#L79-L93)).

**Origin proof on dispatch.** v2 inlines the check at the entrypoint:

```rust
// anchor/lang-v2/derive/src/lib.rs:4510-4546
if __ix_data_len >= 8 {
    let __event_disc: u64 = u64::from_le_bytes(...);
    if __event_disc == EVENT_IX_TAG {
        let __event_authority = __cursor.next();
        if !__event_authority.is_signer() { /* ConstraintSigner */ }
        // recompute find_program_address(&[b"__event_authority"], program_id)
        if !address_eq(...) { /* ConstraintSeeds */ }
        return 0;
    }
}
```

Custom 1-byte ix discriminators that overlap `0xe4` are safe — the
full 8-byte tag is matched intact before user dispatch.

**Feature flag.** v1 needs
`anchor-lang = { features = ["event-cpi"] }`. v2 is always on — no
feature gate ([`anchor/lang-v2/Cargo.toml`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang-v2/Cargo.toml) has no `event-cpi` entry;
`pub fn emit_cpi` declared unconditionally at
[`lang-v2/derive/src/lib.rs:5366`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang-v2/derive/src/lib.rs#L5366)).

**`emit!` vs `emit_cpi!`.**

|                | `emit!`                                       | `emit_cpi!`                                  |
| -------------- | --------------------------------------------- | -------------------------------------------- |
| transport      | `sol_log_data` → `Program data: <b64>` log    | self-CPI; data lives in `meta.innerInstructions` |
| RPC truncation | yes (logs)                                    | no (inner-ix preserved)                      |
| budget         | `LOG_MESSAGES_BYTES_LIMIT` (per-tx)           | per-CPI ix-data cap (10 KiB per call)        |
| CU             | ~syscall base                                 | ~1000 CU + `data_len/250`                    |
| accounts       | none                                          | event_authority + program                    |

`emit!` impl at [`anchor/lang/attribute/event/src/lib.rs:103-111`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang/attribute/event/src/lib.rs#L103-L111).
`emit_cpi!` documentation comment notes "more reliable because RPCs
are less likely to truncate CPI information than program logs".

## 5. Bare self-CPI optimization

**The optimization.** Skip Anchor's `__event_dispatch` entirely.
Use `invoke` (not `invoke_signed`) self-CPI with a
*non-EVENT_IX_TAG* prefix routed via a fallback handler — drop
`event_authority` from the accounts list.

**Minimum account list.** Empty. `invoke_signed`/`invoke` does
require account-info translation through CPI, but the
inner-instruction record is written into `meta.innerInstructions`
regardless of signer status, and the record carries
`programId = caller` unconditionally.

**Origin auth (off-chain).** Indexer filters
`innerInstructions[*].instructions[*]` by
`programIdIndex → eclob_program_id`. The runtime guarantees this
field reflects the actual invoking program — no other program can
fake an inner-ix whose `programId` is the eCLOB program. There's no
on-chain enforcement beyond program-id matching, which is acceptable
for an indexer-only channel.

**Custom prefix tag.** Pick a tag that is NOT
`0x1d9acb512ea545e4` (else Anchor's dispatcher hijacks it). Route via
Anchor's fallback handler or a plain SBF program if you want to skip
Anchor entirely.

**When to flip.** When the router-account-list budget is tight (e.g.,
taker instruction is itself invoked via CPI from a router and adds
30+ account metas, leaving little room before
`MAX_ACCOUNTS_PER_INSTRUCTION = 255`). Each `emit_cpi!` adds 2
account metas; bare drops to 0. Account-meta CU under SIMD-0339 is
`34 B per meta / 250 B/CU ≈ 0.14 CU` — not material for CU, only for
the account-list count.

**CU comparison.** Same `invoke_units` (1000 / 946 CU under
SIMD-0339) + `data_len/250`. Bare saves `2 × 34/250 ≈ 0.27 CU` and 2
account-info translations (`80/250` each under SIMD-0339).
Negligible per-emit; meaningful only at high emit counts or tight
account budgets.

## 6. Runtime limits that govern fidelity

**Per-CPI instruction-data cap: 10,240 bytes.**

```rust
// agave/transaction-context/src/lib.rs:18
pub const MAX_INSTRUCTION_DATA_LEN: usize = 10 * 1024;
```

Enforced at [`agave/program-runtime/src/cpi.rs:147-160`](https://github.com/anza-xyz/agave/blob/1ad187441b53d2ffb8f41a99e06f44ae27fda219/program-runtime/src/cpi.rs#L147-L160) in
`check_instruction_size`. After `EVENT_IX_TAG_LE` (8 B) + event disc
(8 B) = 16 B of overhead, payload budget per emit =
**10,224 bytes**.

**`LOG_MESSAGES_BYTES_LIMIT` does NOT apply to inner-ix.** Verified.

```rust
// agave/svm-log-collector/src/lib.rs:5
const LOG_MESSAGES_BYTES_LIMIT: usize = 10 * 1000;
```

`LogCollector` only touches `messages: Vec<String>`
([`lib.rs:26-42`](https://github.com/anza-xyz/agave/blob/1ad187441b53d2ffb8f41a99e06f44ae27fda219/svm-log-collector/src/lib.rs#L26-L42)). Inner instructions take a separate path:
`TransactionContext.instruction_trace` → `deconstruct_transaction`
([`agave/svm/src/transaction_processor.rs:1089-1139`](https://github.com/anza-xyz/agave/blob/1ad187441b53d2ffb8f41a99e06f44ae27fda219/svm/src/transaction_processor.rs#L1089-L1139)). The log
collector is never consulted.

**Max CPI stack depth: 5 (9 post-SIMD-0268).**

```rust
// agave/program-runtime/src/execution_budget.rs:8-18
pub const MAX_INSTRUCTION_STACK_DEPTH: usize = 5;
pub const MAX_INSTRUCTION_STACK_DEPTH_SIMD_0268: usize = 9;
```

Top-level ix = height 1; each `emit_cpi!` pushes to height 2 then
pops on return. Self-CPI is permitted via the reentrancy guard's
`is_last` check at
[`agave/program-runtime/src/invoke_context.rs:244-263`](https://github.com/anza-xyz/agave/blob/1ad187441b53d2ffb8f41a99e06f44ae27fda219/program-runtime/src/invoke_context.rs#L244-L263). **Design for 5
(pre-SIMD-0268).** Each top-level emission costs 1 level temporarily.

**Per-tx trace length: 64 instructions total.**

```rust
// agave/transaction-context/src/lib.rs:26
pub const MAX_INSTRUCTION_TRACE_LENGTH: usize = 64;
```

This is the hard ceiling on `emit_cpi!` count per transaction. One
top-level eclob ix + N SPL token CPIs leaves `63 - N` emit slots.
**The matching engine MUST aggregate fills into batched event
payloads, not emit one CPI per fill at scale.**

**No cumulative byte cap on inner-ix data.** Confirmed — only the
per-CPI 10 KiB cap and the 64-count cap. Theoretical upper bound
≈ 63 × 10 KiB ≈ 645 KB; real binding constraints are CU budget
(~1000 CU/emit) and the 64 count.

**Why a large sweep splits.** A match that exhausts >10,224 B of
fill data in one `emit_cpi!` MUST be packed across multiple CPIs.
Build the next CPI when adding the next leg would exceed
`10_224 - cursor`. Cap emits to keep
`total_emits + non_event_cpis + 1 ≤ 64`.

## 7. Architecture-spec coherence checks

| Spec claim (line)                                                                | Status                       | Notes                                                                                                                                                                                                                                  |
| -------------------------------------------------------------------------------- | ---------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Ephemeral book on SVM program heap (1375)                                        | **CONFIRMED**                | Default 32 KiB more than covers 10-vault × 32-level case (≈7.7 KiB).                                                                                                                                                                   |
| Min-heap on `(Price::MAX − price, nonce)` for bids (1405-1418)                   | **CONFIRMED — with refinement** | Use `Price::bid_key()` (already at [`price.rs:228-234`](https://github.com/DASMAC-com/dropset/blob/ecfc46fd5e8c627292aefd627899cd05cb28df61/programs/dropset/src/price.rs#L228-L234)) producing `u32`, packed into a `#[repr(C)]` `HeapEntry` whose derived `Ord` is lexicographic. Avoid storing `Price` directly + flipped comparator.                              |
| Inner-ix data not subject to `LOG_MESSAGES_BYTES_LIMIT` (1477-1479)              | **CONFIRMED**                | LogCollector path ([`svm-log-collector/src/lib.rs:5`](https://github.com/anza-xyz/agave/blob/1ad187441b53d2ffb8f41a99e06f44ae27fda219/svm-log-collector/src/lib.rs#L5), [`:26-42`](https://github.com/anza-xyz/agave/blob/1ad187441b53d2ffb8f41a99e06f44ae27fda219/svm-log-collector/src/lib.rs#L26-L42)) is wholly separate from `instruction_trace` ([`transaction_processor.rs:1089-1139`](https://github.com/anza-xyz/agave/blob/1ad187441b53d2ffb8f41a99e06f44ae27fda219/svm/src/transaction_processor.rs#L1089-L1139)).                                                                                         |
| `emit_cpi!` appends `event_authority` and `program` accounts (1483-1485)         | **CONFIRMED**                | [`lang-v2/derive/src/lib.rs:5459-5470`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang-v2/derive/src/lib.rs#L5459-L5470). Order is `event_authority` then `program`.                                                                                                                                                      |
| Bare self-CPI carries event in ix-data, drops `event_authority` (1496-1500)      | **CONFIRMED**                | Off-chain origin proof reduces to `programId == self`. Acceptable for indexer-only channel. Must use a non-`EVENT_IX_TAG` prefix to avoid Anchor's dispatcher hijack.                                                                  |
| No cumulative cap on inner-ix data across a tx (1528-1530)                       | **CONFIRMED**                | Only the per-CPI 10 KiB and the 64-instruction trace count constrain.                                                                                                                                                                  |
| Single CPI ix-data ~10 KB (1528)                                                 | **CONFIRMED**                | Exactly 10,240 B ([`transaction-context/src/lib.rs:18`](https://github.com/anza-xyz/agave/blob/1ad187441b53d2ffb8f41a99e06f44ae27fda219/transaction-context/src/lib.rs#L18)). Effective event-payload budget = 10,224 B after 16 B tag+disc overhead.                                                                                                        |

**Add to spec (not currently called out):**

- Per-tx trace length cap of **64** is the binding ceiling on emit
  count, not the per-CPI cap. Budget emits against
  `64 - (top_level + token_cpis)`.
- Each `emit_cpi!` adds `~1000 CU + data_len/250` CU. At ~120 B/event
  ≈ 1000.5 CU/emit. 200 emits ≈ 200 KCU.

## 8. Open questions / things to verify before implementing

**Downgraded confidence (medium):**

1. **Per-entry size of 24 B (medium).** The architecture spec tuple
   `(price, size, stamp & !FLUSH_BIT, vault_ptr, level_idx)` lands
   at 24-32 B depending on field order. **Verify with
   `core::mem::size_of::<HeapEntry>()`** when the struct is
   concrete. Use `#[repr(C)]` and
   `assert_eq!(size_of::<HeapEntry>(), 24)` as a build-time check.
2. **Pinocchio default-allocator behavior with `RequestHeapFrame`.**
   Pinocchio's `default_allocator!` initializes to
   `MAX_HEAP_LENGTH (256 KiB)` per its source. Confirm by reading
   the actual Pinocchio version pinned in the dropset workspace
   whether allocations beyond 32 KiB succeed without
   `RequestHeapFrame` (likely they fault — Pinocchio assumes the
   frame was requested).
3. **Bare self-CPI under v2's dispatcher.** v2's entrypoint matches
   the full 8-byte `EVENT_IX_TAG` before user dispatch
   ([`lang-v2/derive/src/lib.rs:4510-4546`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang-v2/derive/src/lib.rs#L4510-L4546)). A bare self-CPI must use
   a prefix where the first 8 bytes ≠ `0x1d9acb512ea545e4`.
   **Verify with a unit test** that a chosen bare-tag is routed to
   the fallback handler.

**Where docs lag the code:**

- Solana docs claim a 32 KiB default heap "fits the BumpAllocator"
  but the *solana-program* allocator is hard-pinned to 32 KiB
  regardless of `RequestHeapFrame`
  ([`solana-program-entrypoint-3.1.1/src/lib.rs:219-230`](https://github.com/anza-xyz/solana-sdk/blob/program-entrypoint%40v3.1.1/program-entrypoint/src/lib.rs#L219-L230)). Only
  Pinocchio's allocator (which Anchor v2 installs) reads the actual
  frame size. Don't trust generic Solana guides on heap-sizing for
  v2 programs.
- SIMD-0268 (5 → 9 stack depth) and SIMD-0339 (CPI account-info
  limit + 946 CU base) are activated/in-review and change CU math at
  the margin. Design for pre-SIMD-0268 numbers (5 depth, 1000 CU)
  and treat post-activation as headroom.

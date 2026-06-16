<!-- markdownlint-disable MD013 -->

# SVM heap + Anchor event-cpi: deep-research synthesis

Source-grounded notes feeding the **Order matching** and **Events and
emission** sections of `docs/architecture.md`. Every load-bearing
claim cites a GitHub permalink to a specific file:line range in
`anza-xyz/agave`, `anza-xyz/sbpf`, `anza-xyz/pinocchio`,
`solana-foundation/anchor`, or a published crate at a pinned tag.

> **Status (reconciled to shipped code).** The matching engine shipped
> as a **flat `Vec` sorted once** (`swap.rs`: `Vec<HeapEntry>` +
> `sort_by_key`), **not** the `BinaryHeap`/`pop_min` design these
> notes originally recommended. At the shipped `N_LEVELS = 8`
> (`state/market.rs`), the whole book is small enough that a single
> sort is simpler and costs no meaningful CU, and the spec's
> price-time priority falls out of one `sort_by_key`. The
> `BinaryHeap` analysis below (§2 idioms, §3 recipe) is kept as a
> **considered-and-rejected** alternative — the heap-mechanics,
> `emit_cpi!`, and runtime-limit research in §1 and §4–§6 stays
> authoritative. The `MAX_LEVELS_PER_SIDE` constant referenced by the
> original capacity math was never added; reads as `N_LEVELS = 8`.

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

```text
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
USE the extra bytes. Pinocchio's `default_allocator!` macro is **also
not parameterized** — it hard-codes `MAX_HEAP_LENGTH = 256 KiB` as the
allocator end
([`pinocchio/sdk/src/entrypoint/mod.rs:610-621`](https://github.com/anza-xyz/pinocchio/blob/009301423f920fd105bd32a25560d127b6f0bf4f/sdk/src/entrypoint/mod.rs#L610-L621)).
The allocator hands out addresses up to 256 KiB regardless of the
runtime-requested frame; allocations above the actually-mapped frame
succeed at the allocator level but fault on access. For the default
32 KiB frame this is invisible (Dropset's usage stays well below);
above 32 KiB you must call `RequestHeapFrame` so the VM maps the
region.

**Legacy `sol_alloc_free_` is dead** for new deployments
([`agave/syscalls/src/lib.rs:476-481`](https://github.com/anza-xyz/agave/blob/1ad187441b53d2ffb8f41a99e06f44ae27fda219/syscalls/src/lib.rs#L476-L481), gated by
`disable_deploy_of_alloc_free_syscall`). Ignore.

## 2. Stack vs heap for the ephemeral book

**Stack constraints.** Default SBPF frame = 4096 B, max call
depth = 64, total stack = 256 KiB ([`sbpf/src/vm.rs:107-110`](https://github.com/anza-xyz/sbpf/blob/239cb0bb771224bc49ca679c6a93ee7a876e8cbc/src/vm.rs#L107-L110),
[`vm.rs:99-101`](https://github.com/anza-xyz/sbpf/blob/239cb0bb771224bc49ca679c6a93ee7a876e8cbc/src/vm.rs#L99-L101)). Any single allocation > ~3 KiB in a frame risks
overflow; recursion bounded at 64.

→ **The ephemeral book is heap-allocated.** It's a dynamically-sized
`Vec` whose length isn't known until the active-DLL walk finishes, and
at the `max_vaults_per_market` ceiling it reaches ~64 KiB
(255 × `N_LEVELS = 8` × 32 B) — far past the 4 KiB frame. Even at the
default cap (~2.5 KiB, see below) a stack buffer would have to be
worst-case-sized and would crowd the frame's other locals, so the heap
is the right home regardless.

**Allocation idioms ranked.**

| idiom                          | when                                                                                                                                                                                                                                            |
| ------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Vec` + `sort_by_key`          | **the shipped matching engine** — collect all live levels, sort once; cheapest when the whole book fits in one buffer                                                                                                                           |
| `BinaryHeap::with_capacity(N)` | incremental `pop_min` without a full sort; considered for the matcher but not shipped (a single sort is simpler at `N_LEVELS = 8`)                                                                                                              |
| `Box<[T; N]>`                  | fixed-size slab, no pop-min semantics; you'd reimplement sift-down                                                                                                                                                                              |
| `Vec::with_capacity(N)`        | pre-sized collect-then-sort — avoids the realloc churn of `Vec::new()` if the book ever grows large                                                                                                                                             |
| `Vec::new()` + push            | how the shipped `Vec` above is grown — starts empty and doubles; each doubling leaks the old buffer on the bump allocator, tolerable only because the count is small and bounded (≤ `max_vaults_per_market × N_LEVELS`, ~80 at the default cap) |

**Capacity math.** The shipped `HeapEntry` (`swap.rs`) carries
`(price_key:u32, price:Price, nonce:u64, sector_idx:u32, level_idx:u32, size:u64)` — roughly **32 B/entry** at 8-byte
alignment (the original research used a tighter
`(key, nonce, size, vault_idx:u16, level_idx:u8)` tuple, also ~32 B
under `#[repr(C)]`). The book is **`N_LEVELS = 8`** levels/side
(`state/market.rs`), not the 32/side the original draft assumed. At
`DEFAULT_MAX_VAULTS_PER_MARKET = 10`
([`programs/dropset/src/state/registry.rs:19`](https://github.com/DASMAC-com/dropset/blob/399148c5025044c88bd3fa7f6f3b5a941e8af4ac/programs/dropset/src/state/registry.rs#L19))
× 8 levels/side = 80 entries × 32 B = **2,560 B** — comfortably
inside the 32 KiB default heap even with the `Vec::new()` doubling
churn (peak live + leaked ≈ a few KiB).

**When to `request_heap_frame`.** Not needed at the default cap.
`max_vaults_per_market` is a `u8`
([`programs/dropset/src/state/registry.rs:67`](https://github.com/DASMAC-com/dropset/blob/399148c5025044c88bd3fa7f6f3b5a941e8af4ac/programs/dropset/src/state/registry.rs#L67));
even the worst case of 255 × 8 levels/side × 32 B ≈ 64 KiB would
exceed the 32 KiB default, but the realistic cap (10 vaults) sits at
~2.5 KiB. Only a market configured near the `u8` ceiling would need a
frame request (8 CU per extra 32 KiB), and the `Vec::new()` doubling
would warrant pre-sizing with `Vec::with_capacity` at that point.

**v2 import note.** `lang-v2` is `#![no_std]`
([`anchor/lang-v2/src/lib.rs:5-6`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang-v2/src/lib.rs#L5-L6)). The shipped matcher uses
`alloc::vec::Vec`; a heap variant would use
`alloc::collections::BinaryHeap`, not the `std` one.

## 3. The matching engine on the heap (concrete recipe)

**The keyed-entry shape (shipped).** Collect every live level into a
flat `Vec`, translating the price to a `u32` sort key at push time so
a single `sort_by_key` orders both sides without a per-compare
branch:

```rust
extern crate alloc;

#[derive(Copy, Clone)]
struct HeapEntry {
    price_key: u32,  // ask: price.as_u32(); bid: price.bid_key()
    price: Price,    // original price, kept for the fill math
    nonce: u64,      // (stamp & !FLUSH_BIT) — masked at push
    sector_idx: u32,
    level_idx: u32,
    size: u64,
}

let mut heap: alloc::vec::Vec<HeapEntry> = alloc::vec::Vec::new();
// ... walk the active DLL, push one entry per live level ...
heap.sort_by_key(|e| (e.price_key, e.nonce, e.sector_idx, e.level_idx));
```

`Price::bid_key()` already exists at
[`sdk/price-core/src/price.rs:251-257`](https://github.com/DASMAC-com/dropset/blob/fac006594ae37b61f46cd7d5429ea4b4ad07d857/sdk/price-core/src/price.rs#L251-L257) and maps the highest bid
price to the lowest `u32`, so the ascending `sort_by_key` puts the
best price first on both sides; asks use `as_u32()` directly. The
struct keeps the original `Price` alongside the key because the fill
loop needs it for the quote/base conversion — the sort reads
`price_key`, not `price`, so the "key inversion" pitfall below is
sidestepped. `sector_idx` / `level_idx` are appended to the sort key
as a deterministic final tiebreak after `(price_key, nonce)`.

**Sizing.** The shipped code starts from `Vec::new()` and lets it
grow. At `N_LEVELS = 8` the entry count is bounded at
`max_vaults_per_market × 8` (~80 at the default cap), so the doubling
churn stays within a few KiB of the 32 KiB heap. If the engine ever
matched a much larger book, switch to
`Vec::with_capacity(vaults × N_LEVELS)` for a single up-front
allocation — the bump allocator never reclaims a doubled-away buffer
([`pinocchio/sdk/src/entrypoint/mod.rs:832-833`](https://github.com/anza-xyz/pinocchio/blob/009301423f920fd105bd32a25560d127b6f0bf4f/sdk/src/entrypoint/mod.rs#L832-L833)).

**Why a sorted `Vec` over a `BinaryHeap` (considered, rejected).** A
min-heap with `pop_min` avoids materializing a fully-sorted order
when the taker consumes only the first few levels. At `N_LEVELS = 8`
the book is tiny, the taker often sweeps most of it, and a flat sort
is simpler to read and free of the `Reverse`/comparator bookkeeping —
so the heap was dropped in favor of one `sort_by_key`. Revisit if
`N_LEVELS` or `max_vaults_per_market` grow by an order of magnitude.

**Tear-down.** The `Vec` is dropped at instruction return when the VM
tears down the entire heap region. No explicit `dealloc` is run
(bump allocator's `dealloc` is a no-op anyway). Zero work; zero CU.

**Pitfalls.**

- **Grow-time realloc leak.** Each `Vec` doubling permanently
  consumes `old + new` bytes for the rest of the instruction; bounded
  but real. Pre-size with `Vec::with_capacity` if the book grows.
- **Key inversion mistake.** Don't sort on `Price` directly and flip
  the comparator — translate to a `u32` key (`as_u32()` / `bid_key()`)
  at push time and sort on the key, as the shipped `HeapEntry` does.
- **FLUSH_BIT masking.** Per **architecture.md → Order matching →
  Book construction**: mask `stamp & !FLUSH_BIT` at push time (when
  the `nonce` key is captured), not at compare time.
- **Stack pressure from event literal.** `emit_cpi!(FillEvent { .. })`
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

```text
[ 8 B EVENT_IX_TAG_LE = e4 45 a5 2e 51 cb 9a 1d ]   // u64 0x1d9acb512ea545e4 to_le_bytes;
                                                    // the u64 itself is sha256("anchor:event")[..8] read BE
[ 8 B event-discriminator = sha256("event:<EventName>")[..8] ]
[ event body — wincode (borsh-wire-compatible) by default,    ]
[ or bytemuck::bytes_of(self) under #[event(bytemuck)]        ]
```

- `EVENT_IX_TAG = 0x1d9acb512ea545e4` at [`anchor/lang/src/event.rs:1-3`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang/src/event.rs#L1-L3)
- Event disc generator at
  [`anchor/lang/attribute/event/src/lib.rs:42-44`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang/attribute/event/src/lib.rs#L42-L44) and
  [`lang/syn/src/codegen/program/common.rs:11-16`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang/syn/src/codegen/program/common.rs#L11-L16)
- Per-event `Event::data` preallocates 256 B (was 1024 pre-0.30):
  [`anchor/lang/attribute/event/src/lib.rs:50-56`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang/attribute/event/src/lib.rs#L50-L56)

**Serialization modes (v2-only).** v2 picks the serializer via the
`#[event]` macro argument
([`lang-v2/derive/src/lib.rs:4915`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang-v2/derive/src/lib.rs#L4915)):

| mode                 | serializer / writer                                                  | constraints                                                 | when to pick                                                                             |
| -------------------- | -------------------------------------------------------------------- | ----------------------------------------------------------- | ---------------------------------------------------------------------------------------- |
| `#[event]` (default) | wincode with `BORSH_CONFIG` — wire output is byte-identical to borsh | supports `Vec` / `String` / `Option` / nested enums         | cold-path events that need dynamic shape (`OpenVault`, `Realize`, `Deposit`, `Withdraw`) |
| `#[event(bytemuck)]` | zero-copy `bytemuck::bytes_of(self)` on a `repr(C)` POD struct       | fixed-size primitives only; no fat pointers, no allocations | hot-path fill events — cheapest emit, smallest stack footprint at the macro site         |

- Wincode branch at
  [`lang-v2/derive/src/lib.rs:3206-3214`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang-v2/derive/src/lib.rs#L3206-L3214)
- Bytemuck branch at
  [`lang-v2/derive/src/lib.rs:3185-3196`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang-v2/derive/src/lib.rs#L3185-L3196)
- IDL `TypeKind::Borsh` vs `TypeKind::BytemuckRepr` tagging at
  [`lang-v2/derive/src/lib.rs:4949-4955`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang-v2/derive/src/lib.rs#L4949-L4955)

Because the default-mode wire format is borsh-identical via
`BORSH_CONFIG`, off-chain decoders that already use borsh keep working
unchanged for default events. Bytemuck events are decoded as a
`repr(C)` blob in the IDL (`{serialization:"bytemuck",repr:{kind:"c"}}`)
and require the indexer to read fields by offset.

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

Caller passes them as `(event_authority_pda, is_signer=false, is_writable=false)` and `(program_id, false, false)`. The program
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

|                | `emit!`                                    | `emit_cpi!`                                      |
| -------------- | ------------------------------------------ | ------------------------------------------------ |
| transport      | `sol_log_data` → `Program data: <b64>` log | self-CPI; data lives in `meta.innerInstructions` |
| RPC truncation | yes (logs)                                 | no (inner-ix preserved)                          |
| budget         | `LOG_MESSAGES_BYTES_LIMIT` (per-tx)        | per-CPI ix-data cap (10 KiB per call)            |
| CU             | ~syscall base                              | ~1000 CU + `data_len/250`                        |
| accounts       | none                                       | event_authority + program                        |

`emit!` impl at [`anchor/lang/attribute/event/src/lib.rs:103-111`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang/attribute/event/src/lib.rs#L103-L111).
`emit_cpi!` documentation comment notes "more reliable because RPCs
are less likely to truncate CPI information than program logs".

## 5. Bare self-CPI optimization

**The optimization.** Skip Anchor's `__event_dispatch` entirely.
Use `invoke` (not `invoke_signed`) self-CPI with a
*non-EVENT_IX_TAG* prefix routed via a fallback handler — drop
`event_authority` from the accounts list.

**Minimum account list.** One — the `program` account stays on the
outer Accounts struct (any self-CPI requires the callee program's
`AccountInfo` to be available). Inner-instruction records land in
`meta.innerInstructions` regardless of signer status; `programId = caller` always.

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

**When to flip.** When the router-account-list budget is tight
(e.g., the taker instruction is itself invoked via CPI from a router
that adds 30+ account metas, leaving little room before
`MAX_ACCOUNTS_PER_INSTRUCTION = 255`). `#[event_cpi]` injects 2
accounts into the outer Accounts struct (`event_authority` +
`program`); bare drops `event_authority`, keeping `program` —
**saves 1 outer-account slot**. Account-meta CU under SIMD-0339 is
`34 B per meta / 250 B/CU ≈ 0.14 CU` — not material for CU, only
for the account-list count.

**CU comparison.** Same `invoke_units` (1000 / 946 CU under
SIMD-0339) + `data_len/250`. Inside the self-CPI itself the
`event_authority` is the only signer meta Anchor pushes — bare
drops it for `1 × 34/250 ≈ 0.14 CU` and one account-info
translation (`80/250` under SIMD-0339). Negligible per-emit;
meaningful only at high emit counts or tight account budgets.

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

| Spec claim (architecture.md section)                                                      | Status                                     | Notes                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| ----------------------------------------------------------------------------------------- | ------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Ephemeral book on SVM program heap (**Order matching**)                                   | **CONFIRMED**                              | Default 32 KiB heap covers the 10-vault × `N_LEVELS = 8` case at the §3 entry layout (~2.5 KiB; wide margin).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| Cross-vault price-time priority for bids (**Order matching → Book construction**)         | **CONFIRMED — shipped as a sorted `Vec`**  | Shipped builds a flat `Vec<HeapEntry>` and runs one `sort_by_key((price_key, nonce, sector_idx, level_idx))`, where `price_key` is `price.bid_key()` for bids / `price.as_u32()` for asks ([`sdk/price-core/src/matching.rs:114-127`](https://github.com/DASMAC-com/dropset/blob/fac006594ae37b61f46cd7d5429ea4b4ad07d857/sdk/price-core/src/matching.rs#L114-L127) builds the flat `Vec<Lvl>` and runs the sort; the `sort_key` comparator lives at [`sdk/price-core/src/matching_math.rs:66-72`](https://github.com/DASMAC-com/dropset/blob/fac006594ae37b61f46cd7d5429ea4b4ad07d857/sdk/price-core/src/matching_math.rs#L66-L72)) — **not** the `BinaryHeap`/`pop_min` originally drafted in §2–§3, which is kept there as a rejected alternative. The `bid_key` trick still keeps both sides on one ascending comparator. |
| Inner-ix data not subject to `LOG_MESSAGES_BYTES_LIMIT` (**Events and emission**)         | **CONFIRMED**                              | LogCollector path ([`svm-log-collector/src/lib.rs:5`](https://github.com/anza-xyz/agave/blob/1ad187441b53d2ffb8f41a99e06f44ae27fda219/svm-log-collector/src/lib.rs#L5), [`:26-42`](https://github.com/anza-xyz/agave/blob/1ad187441b53d2ffb8f41a99e06f44ae27fda219/svm-log-collector/src/lib.rs#L26-L42)) is wholly separate from `instruction_trace` ([`transaction_processor.rs:1089-1139`](https://github.com/anza-xyz/agave/blob/1ad187441b53d2ffb8f41a99e06f44ae27fda219/svm/src/transaction_processor.rs#L1089-L1139)).                                                                                                                                                                                                                                                                                                 |
| `emit_cpi!` appends `event_authority` and `program` accounts (**Events and emission**)    | **CONFIRMED — outer Accounts struct only** | [`lang-v2/derive/src/lib.rs:5459-5470`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang-v2/derive/src/lib.rs#L5459-L5470) — order is `event_authority` then `program`. The self-CPI invocation itself takes only `event_authority` as a readonly signer ([`lang-v2/derive/src/lib.rs:5370-5395`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang-v2/derive/src/lib.rs#L5370-L5395)); `program` is on the outer struct so the runtime can supply its `AccountInfo` to the invoke.                                                                                                                                                                                                                                                |
| Bare self-CPI carries event in ix-data, drops `event_authority` (**Events and emission**) | **CONFIRMED**                              | Off-chain origin proof reduces to `programId == self`. Acceptable for indexer-only channel. Must use a non-`EVENT_IX_TAG` prefix to avoid Anchor's dispatcher hijack.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| No cumulative cap on inner-ix data across a tx (**Events and emission**)                  | **CONFIRMED**                              | Only the per-CPI 10 KiB and the 64-instruction trace count constrain.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| Single CPI ix-data ~10 KB (**Events and emission**)                                       | **CONFIRMED**                              | Exactly 10,240 B ([`transaction-context/src/lib.rs:18`](https://github.com/anza-xyz/agave/blob/1ad187441b53d2ffb8f41a99e06f44ae27fda219/transaction-context/src/lib.rs#L18)). Effective event-payload budget = 10,224 B after 16 B tag+disc overhead.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |

**Add to spec (not currently called out):**

- Per-tx trace length cap of **64** is the binding ceiling on emit
  count, not the per-CPI cap. Budget emits against
  `64 - (top_level + token_cpis)`.
- Each `emit_cpi!` adds `~1000 CU + data_len/250` CU. At ~120 B/event
  ≈ 1000.5 CU/emit. 200 emits ≈ 200 KCU.
- **Event serialization mode.** Use `#[event(bytemuck)]` for the
  fill event — the payload is fixed-size primitives (taker pubkey,
  leader pubkey, quote_authority pubkey, fill amounts, price,
  post-fill inventory), it's emitted per leg on the hot path, and
  bytemuck cuts both the serializer cost and the stack footprint of
  the struct literal at the macro site (Section 3 pitfall). Use the
  default `#[event]` (wincode, borsh-wire-compatible) for cold-path
  events that benefit from `Vec` / `String` / `Option` — `OpenVault`,
  `Realize`, `Deposit`, `Withdraw`.

## 8. Open questions / things to verify before implementing

**To verify before implementing:**

1. **`MAX_LEVELS_PER_SIDE` constant — RESOLVED.** The original
   capacity math in §2 and recipe in §3 referenced
   `MAX_LEVELS_PER_SIDE` (assumed 32). No such constant was ever
   added; the program ships `N_LEVELS = 8` (`state/market.rs`) as the
   per-side level count, and the matcher is a sorted `Vec`, not a
   capacity-sized `BinaryHeap`, so there is no `with_capacity` arg to
   pin. The book is ≤ `max_vaults_per_market × N_LEVELS` (~80 at the
   default cap of 10), ~2.5 KiB — well inside the 32 KiB heap, so no
   build-time heap-fit assertion is wired. Use `N_LEVELS` anywhere the
   old notes say `MAX_LEVELS_PER_SIDE`.
1. **Bare self-CPI under v2's dispatcher.** v2's entrypoint matches
   the full 8-byte `EVENT_IX_TAG` before user dispatch
   ([`lang-v2/derive/src/lib.rs:4510-4546`](https://github.com/solana-foundation/anchor/blob/2a191379020f15c1d384bdadd41f23949734ce98/lang-v2/derive/src/lib.rs#L4510-L4546)). A bare self-CPI must use
   a prefix where the first 8 bytes ≠ `0x1d9acb512ea545e4`.
   **Verify with a unit test** that a chosen bare-tag is routed to
   the fallback handler.
1. **SIMD-0268 / SIMD-0339 activation status.** The doc designs for
   pre-SIMD-0268 numbers (stack depth 5, invoke 1000 CU). Before
   relying on the higher post-SIMD numbers, check the current
   Agave feature-gate set against the cluster you're targeting.

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

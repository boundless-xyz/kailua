## Context

Kailua proves OP-Stack rollup state transitions using RISC Zero's zkVM. The existing architecture proves blocks monolithically: every transaction in a block executes inside a single zkVM guest execution, producing a `ProofJournal` that attests to the state transition. Blocks are composed via a stitching system that chains `ProofJournal` entries and verifies them recursively via `env::verify()`.

The execution pipeline is:

1. **Host**: Pre-executes blocks, assembles witnesses with preimage data.
2. **Guest**: Executes blocks statelessly via `StatelessL2Builder::build_block()`, using `CachedExecutor` to leverage pre-computed results.
3. **Stitching**: `stitch_executions()` and `stitch_boot_info()` compose multiple block proofs into range proofs.

The `Precondition` struct tracks proof assumptions with four existing fields: `proposal_blobs`, `execution_trace`, `derivation_cache`, `derivation_trace`. The `Digestible` implementation dispatches between execution-only (`execution_trace != 0`) and combined derivation/proposal modes.

The guest programs (`kailua-fpvm-kona`, `kailua-fpvm-hana`, `kailua-fpvm-hokulea`) share the same execution flow via the `StitchingClient` trait and always emit a 220-byte packed `ProofJournal`.

## Goals / Non-Goals

**Goals:**
- Reduce proving latency for blocks with expensive transactions by enabling parallel proving of transaction subsequences ("partial executions").
- Integrate cleanly into the existing `ProofJournal`, `Precondition`, and stitching systems — partial proofs emit standard `ProofJournal`s, block aggregation produces standard `BlockBuildingOutcome`s.
- Zero modifications to vendored code in `optimism/rust/`.
- Full backward compatibility — disabled by default (empty `partial_executions` + `pe_witness = None`, `--num-block-partials 0`).

**Non-Goals:**
- Automatic partition sizing based on transaction cost estimation. (Splitter is uniform: `tx_count.div_ceil(num_block_partials).max(1)` per slice.)
- State-access conflict analysis for optimal parallelism (partials are sequential slices of the tx body).
- Cross-block transaction partitioning (partials are always within a single block).
- Modifying the on-chain verification contracts or fault-proof game.

## Decisions

### Decision 1: One unified `CachedEvm` / `CachedEvmFactory` wrapper serves all roles

**Choice:** A single `Evm`-trait wrapper (`CachedEvm<E: Evm>`, in `crates/kona/src/evm/cached.rs`) replaces what earlier iterations split across four types (tracing evm + factory, chunking evm + factory). Depending on construction it serves as:

1. **Transparent pass-through** — empty partial cache, no collector → every method delegates to the inner EVM.
2. **Trace collector** — empty cache + shared `TransactionResultCollector` → every successful fall-through `transact_raw` appends `PartialExecutionTrace { tx_hash, result, expected_state }` to the last inner `Vec` of the collector. The `expected_state` snapshot is captured *before* the inner `transact_raw` call (so it reflects the pre-tx live DB view).
3. **Cached serve** — populated `Vec<PartialExecution>` (reversed) → `transact_raw` matches the incoming tx hash against the top partial's next `tx_hashes.last()`. Match → on first cached serve in this partial, assert `expected_state` against the live DB; for every served result, authenticate `original_info` / `original_value`; then return cached result. Miss → fall through to the inner EVM (and, if a collector is attached, capture the fresh execution).

The `CachedEvmFactory` keeps per-block partials in a reversed `Arc<Mutex<Vec<Vec<PartialExecution>>>>` and pops one block's worth per `create_evm` call. It also pushes a fresh per-EVM slot onto the shared trace collector before each EVM creation.

**Alternatives considered:**
- *Separate `TracingOpEvm` and `ChunkingEvm`.* Earlier design. Rejected because the two share >90% of their logic; keeping them separate would cost a factory split, two test suites, and a feature-gated `prover` crate boundary for no architectural benefit.
- *Modify `build_block()` to accept a hook.* Rejected — modifies vendored code.
- *Inspector-based tracing.* Rejected — wrong granularity (instruction-level vs tx-level), higher overhead.

**Rationale:** The `EvmFactory` is a generic parameter throughout `KonaExecutor<P, H, Evm>` and `StatelessL2Builder<P, H, Evm>`. Replacing it requires only changing `CachedExecutor::new()` generics — the vendored generic plumbing carries our factory through unchanged. Unifying to one type halves the surface we need to maintain and collapses the "tracing" and "aggregation" code paths in `run_core_client` into a single executor construction.

### Decision 2: Partial proofs reuse `ProofJournal` with a sentinel `l1_head`

**Choice:** Partial proofs emit the standard 220-byte `ProofJournal` with `l1_head = B256::repeat_byte(0xFF)` and `agreed_l2_output_root == claimed_l2_output_root`.

| Field | Value |
|-------|-------|
| `payout_recipient` | Identical to block proof |
| `precondition_hash` | `digest(Precondition::default().partial(pe_trace))` (which is `Digest::from_bytes(pe_trace.0)`) |
| `l1_head` | `0xFF..FF` (partial sentinel) |
| `agreed_l2_output_root` | `partial.op_block_ctx.parent_hash` |
| `claimed_l2_output_root` | Same as agreed (partial does not advance L2 state) |
| `claimed_l2_block_number` | Parent block number (`block_env.number - 1`) |
| `config_hash` | Identical to block proof |
| `fpvm_image_id` | Identical to block proof |

**Rationale:** Reusing `ProofJournal` means partial receipts flow through the existing `load_stitching_journals()` / `verify_stitching_journal()` infrastructure with zero changes. `l1_head = 0xFF..FF` distinguishes partials from block proofs and execution-only proofs (`l1_head = 0x00..00`). `KonaStitchingClient::run_stitching_client` short-circuits the rest of stitching when it sees the sentinel.

### Decision 3: New `partial_executions` field on `Precondition`

**Choice:** Add `pub partial_executions: B256` as a fifth field on `Precondition`, with a `.partial(hash)` builder method. The `Digestible` impl treats a non-zero `partial_executions` as a third exclusive mode (all other fields must be zero).

**Rationale:** A partial proof commits to a fundamentally different assumption than `execution_trace` (block-level artifacts) or `derivation_cache`/`derivation_trace` (derivation-pipeline state). Overloading an existing field would conflate modes. A dedicated field keeps the assertion logic in `Digestible::digest()` simple and makes intent self-documenting.

### Decision 4: `partial_executions` is a three-input SHA256

**Choice:** `compute_pe_trace(results_hash, block_ctx_hash, expected_state_hash) = SHA256(results_hash || block_ctx_hash || expected_state_hash)`, where:

- `results_hash = hash_results(tx_hashes, results)` — SHA256 over the per-transaction `(tx_hash, PartialResultAndState)` pairs the partial guest captured. The canonical encoding includes each `PartialResultAndState`'s `ExecutionResult` (variant tag, gas values, logs, output bytes) AND its sorted `state` Vec (per-address, per-slot: `original_info` + `info` + `status` bitflags + storage slots' `original_value` / `present_value`). Transient fields (`transaction_id`, `is_cold`, `AccountInfo.code`) are excluded.
- `block_ctx_hash = hash_block_ctx(block_env, op_block_ctx)` — SHA256 over every `BlockEnv` field and every `OpBlockExecutionCtx` field.
- `expected_state_hash = hash_expected_state(expected_state)` — SHA256 over the OP-specific snapshot (`L1_BLOCK_CONTRACT` × six known slots).

**Why three inputs (vs. two):** `results_hash` binds the per-tx state diff, but the OP block executor reads L1Block contract storage *outside* the per-tx diff during fee calculation. A two-input commitment (`results_hash + block_ctx_hash`) cannot bind those reads — an adversary could prove a partial against a forged L1Block view that yields valid per-tx results. The third input commits to the spec-bounded set of fee-related slots at the partial's *starting* prestate; combined with the per-tx prestate authentication in `CachedEvm`, this closes the gap.

A bonus of folding `original_info` into `results_hash`: removing every other hash chain — `hash_cache`, `hash_cache_state`, `hash_overlay_state`, `EvmAccumulatorState`, `hash_evm_state`, `verify_block_chunks`, `PanicDB`, and the rkyv mirror types from earlier drafts — along with all their round-trip tests.

### Decision 5: Per-transaction prestate authentication inside `CachedEvm::transact_raw`

**Choice:** When `CachedEvm::transact_raw` serves a cached `ResultAndState`, BEFORE returning it the wrapper:

1. Asserts `chunk.block_env == self.evm.block()` (rejects forged block context against the aggregation-side live env).
2. On first cached serve in this partial only (gated by `ActivePartialExecution::expected_state_verified`): asserts `capture_required_expected_state(db) == chunk.partial.expected_state`. Once verified the flag flips so subsequent serves skip this O(slots) check.
3. For every `(addr, account)` in the cached `state`:
    - Calls `Database::basic(db, addr)`. Asserts `stored_info == *account.original_info` (or requires `Created` / `LoadedAsNotExisting` status when `None`).
    - For every `(slot, evm_slot)`, calls `Database::storage(db, addr, slot)`. Asserts `actual == evm_slot.original_value`.

The `db` is the same `State<TrieDB>` the aggregation path commits final results to, so the reads resolve to the REAL L2 state. Any divergence — wrong account pre-state, wrong slot pre-value, stale `block_env`, mismatched L1Block snapshot — panics before the cached result is handed to the block executor.

**Rationale:** This is the single load-bearing security decision. It replaces three earlier mechanisms (pre/post DB hash continuity, pre/post EVM-accumulator continuity, standalone `verify_block_chunks` cross-check) with one tightly scoped in-EVM check. Because the authentication happens at the exact point the result flows into the rest of the aggregation, it is impossible to smuggle mismatched state through the boundary.

### Decision 6: Partial guests run against a real `State<TrieDB>` seeded with a `CacheState` prestate

**Choice:** The PARTIAL EXECUTION branch in `run_core_client` builds its state via `State::builder().with_database(TrieDB::new(...)).with_cached_prestate(cache).build()` — where `cache: CacheState` comes from the witness. Transactions are dispatched through the standard `OpBlockExecutor::execute_transaction`. There is no `PanicDB`, no accumulator seeding, no skipped pre-execution / finish, no hand-rolled tx loop.

**Alternatives considered:**
- *`CacheDB<PanicDB>`* — Earlier design. Rejected because the partial guest is already supplying a `CacheState` (which is structurally what `State<DB>` uses internally) and the `TrieDB` backing the live state is already available via the oracle. Panicking on missing state was a defensive measure that is now unnecessary — any miss falls through to the trie, which the oracle serves, and the resulting proof is still bound by `results_hash`.
- *Hand-rolled `execute_transaction_without_commit` + `commit_transaction` loop.* Rejected. The standard `execute_transaction` path automatically handles per-tx validation (block-available gas, Jovian DA footprint, L1Block preload), receipt building, deposit-nonce handling, and the commit step. Reproducing those rules is a maintenance tax every OP hardfork compounds.

**Rationale:** Simpler is safer: the partial guest exercises the exact same vendored execution path the monolithic and aggregation proofs use. Upstream OP hardfork changes are inherited for free.

### Decision 7: `PartialExecution` carries five fields, not seven

**Choice:** Five fields — `tx_hashes`, `results: Vec<PartialResultAndState>`, `expected_state: Vec<ExpectedStateEntry>`, `block_env`, `op_block_ctx`. No `tx_count`, no `agreed_db` / `claimed_db` / `agreed_evm` / `claimed_evm` / `evm_state` (the per-tx prestate check provides continuity at aggregation time, not a witness-level hash chain), no `block_number` / `chunk_index` (position in the outer vec is the block number offset, and within a block partials are consumed top-to-bottom by `CachedEvm`).

`PartialExecution` also exposes:
- `precondition_hash() -> B256` — the chunk's three-input `compute_pe_trace`.
- `boot_info(&BootInfo) -> BootInfo` — derives the partial's `BootInfo` from the witness fields plus the parent boot's chain config.
- `split(partials_per_block: usize) -> Vec<PartialExecution>` — slices the partial into `partials_per_block` sub-partials, threading a running `expected_state` snapshot tx-by-tx so each slice carries the correct starting view. Returns empty when `partials_per_block == 0`.

**Rationale:** Each omitted field was removable because `CachedEvm`'s serve-time authentication subsumed its role. The `expected_state` field is the one new addition vs. earlier drafts — required because OP fee logic reads L1Block contract slots outside the per-tx state diff.

### Decision 8: `PartialResultAndState` is a sorted-Vec mirror of revm's `ResultAndState`

**Choice:** Define `PartialResultAndState { result: ExecutionResult<OpHaltReason>, state: Vec<PartialStateEntry> }` (in `crates/kona/src/evm/partial.rs`) with `From<ResultAndState<OpHaltReason>>` (sorts by address) and `From<PartialResultAndState> for ResultAndState<OpHaltReason>` (rebuilds the `HashMap`). `PartialAccount` similarly carries a `Vec<PartialStorageEntry>` sorted by slot.

**Alternatives considered:**
- *Use revm's `ResultAndState` directly with a `BTreeMap` round-trip per hash call.* Rejected — every `hash_results` call would have to clone+sort the state, and rkyv would round-trip the unsorted `HashMap` form (whose iteration order is non-deterministic).
- *Rkyv mirror types in `rkyv/evm.rs` only (no domain Vec form).* Rejected — the canonical hash needs a stable iteration order, which the rkyv archived form has but is awkward to use from non-rkyv code (e.g. unit tests for `hash_results`).

**Rationale:** Keep one shape that's both the canonical-hash input and the rkyv-friendly serialization form. Tests round-trip `ResultAndState ↔ PartialResultAndState` by set-equality of `(addr, slot, original, present)` tuples (the per-account `HashMap → Vec` projection is order-agnostic).

### Decision 9: `ExpectedState` covers a fixed, spec-bounded slot set

**Choice:** `EXPECTED_STATE_ADDRESSES = [L1_BLOCK_CONTRACT]` and `EXPECTED_STORAGE_SLOTS = [L1_BASE_FEE_SLOT, ECOTONE_L1_FEE_SCALARS_SLOT, L1_OVERHEAD_SLOT, L1_SCALAR_SLOT, ECOTONE_L1_BLOB_BASE_FEE_SLOT, OPERATOR_FEE_SCALARS_SLOT]`. `capture_required_expected_state(db)` reads exactly this `(address × slot)` cartesian product and returns the canonicalized `Vec<ExpectedStateEntry>`. `apply_result_to_expected_state` advances a running snapshot per tx — but never inserts new accounts/slots, so the snapshot stays bounded by the spec.

**Why fixed (not derived):** OP fee calculation reads these specific slots in a known sequence. A dynamic "track everything we read" design would (a) require deeper changes to vendored code to inspect ad-hoc reads, (b) inflate the snapshot size, (c) make `hash_expected_state` non-deterministic across hardforks. A fixed slot set per spec is forward-compatible: future hardforks add new slot constants, the array grows, the encoding stays canonical.

**Why `apply_result_to_expected_state` doesn't add new slots:** The mid-block splitter folds the running snapshot through one tx's state diff at a time. If a tx writes to a slot outside `EXPECTED_STORAGE_SLOTS` (e.g. application-contract storage), it has no business polluting the snapshot — application reads go through the normal per-tx diff. Inserting a new slot would inflate later sub-partials beyond what the spec-bounded re-derive produces, breaking the round-trip equality `hash_expected_state(host_capture) == hash_expected_state(guest_re_derive)`.

### Decision 10: Once-per-partial expected-state check (not once-per-tx)

**Choice:** `ActivePartialExecution { partial: PartialExecution, expected_state_verified: bool }` — the flag flips on the first cached serve in the partial. Subsequent serves skip the `capture_required_expected_state(db) == chunk.partial.expected_state` assertion.

**Rationale:** The check costs `EXPECTED_STATE_ADDRESSES.len() × EXPECTED_STORAGE_SLOTS.len()` oracle round-trips (currently 6 storage reads + 1 account read). Running it on every cached tx would multiply that by the partial's tx count — wasted work, since any subsequent divergence between snapshot and live DB would already be caught by the per-tx prestate authentication (which fires every served tx).

The `expected_state` is a partial-start snapshot, not a per-tx invariant: subsequent txs commit changes to the L1Block slots into the live DB normally. Verifying the snapshot once at the boundary is sufficient to bind the partial to the real L1Block view.

### Decision 11: `PartialExecutionWitness` does not ship `expected_state` explicitly

**Choice:** `PartialExecutionWitness { transactions, cache: CacheState, block_env, op_block_ctx }`. The `expected_state` snapshot is folded into `cache` during witness construction by `cache_results`: it loops over `expected_state` first, inserting each entry's `original_info` + storage values into `cache.accounts` (then loops over `partial.results` to add per-tx prestate). The guest re-derives `expected_state` via `capture_required_expected_state(&mut state)` against the seeded `State`, and feeds the result into `hash_expected_state` — which matches the host's pre-hash input by construction.

**Rationale:** Avoids witness duplication. The `cache` already needs to carry pre-state for every account/slot the partial reads; `expected_state` is a strict subset of that view. Shipping it twice would balloon witness size and create a second source of truth for the snapshot.

The splitter ensures `apply_result_to_expected_state` keeps the snapshot bounded by the same spec-bounded slot set the guest re-derives, so the guest's re-derive matches the host's hash input across the round trip (verified by `witness_cache_round_trips_expected_state` test).

### Decision 12: Partial stitching is split between `precompute_pe_boots` and `stitch_partial_executions`

**Choice:** Before `run_core_client` consumes the `partial_executions` Vec, `run_stitching_client` calls `precompute_pe_boots(&partial_executions)` to produce `Vec<(B256, StitchedBootInfo)>` while the partial data is still borrowed. After `run_core_client` returns (and only when not in partial-mode short-circuit), `stitch_partial_executions(&boot, fpvm_image_id, payout_recipient, pe_boots, &proven_fpvm_journals)` reconstructs each partial's `ProofJournal` and verifies it against `proven_fpvm_journals`.

**Rationale:** The two-phase split allows `run_core_client` to consume the partials vector (moving it into `CachedEvmFactory`) while still allowing the stitching step to enumerate what was consumed. It also mirrors the existing `stitch_executions` pattern.

### Decision 13: Unified `run_core_client` path — aggregation = execution with populated partials

**Choice:** Both the `EXECUTION ONLY` and `DERIVATION & EXECUTION` branches construct their executor with `CachedEvmFactory::new_with_traces(partial_executions, partials_collector)`. Empty `partial_executions` = transparent pass-through; populated = cached serve with inline prestate + expected-state authentication. There is no separate "aggregation mode" and no branching on chunk presence.

**Rationale:** Collapsing the two modes into one code path removes an entire class of branching logic — every test for "partial path" also exercises "non-partial path" when the vec is empty. This is what makes the `partial_executions = Vec::new()` default a safe, byte-identical no-op.

### Decision 14: `EvmFactory`-generic `CachedExecutor`

**Choice:** `CachedExecutor::new` takes a generic `Evm: EvmFactory<Spec = OpSpecId, BlockEnv = BlockEnv> + Send + Sync + Clone + Debug + 'static` parameter (with the additional `Tx`-shaped trait bounds required by `KonaExecutor`). The factory flows through to `KonaExecutor::new`. All call sites pass a `CachedEvmFactory`.

**Rationale:** The refactor is purely mechanical — the pre-refactor specialized impl block simply becomes one point in the generic space. It unlocks Decision 1 (one `Evm` wrapper serving multiple roles) without duplicating the executor wiring.

### Decision 15: Prover-side dispatch via `--num-block-partials` and `PartialsCache`

**Choice:** `ProvingArgs::num_block_partials: usize` (default `0` = monolithic). When positive, `prove.rs`:

1. Runs `concurrent_preflight` (which captures one full-block `PartialExecution` per non-empty block alongside the execution trace).
2. For each block-execution + block-partial pair: `partial.split(num_block_partials)` slices the block-wide partial into up to `num_block_partials` sub-partials, threading the running `ExpectedState` snapshot through each tx boundary.
3. For each sub-partial, dispatches a `compute_oneshot_task` job with `BootInfo { l1_head: 0xFF..FF, agreed_l2_output_root = parent_hash, claimed_l2_output_root = parent_hash, claimed_l2_block_number = parent_number }`, `partial_executions = vec![vec![partial]]`, and `stitched_executions = vec![vec![execution]]`. Each sub-partial proof runs in parallel up to `num_concurrent_proofs`.
4. Accumulates partials into `partial_proof_cache: BTreeMap<u64, Vec<PartialExecution>>` (keyed by parent block number).
5. The aggregation pass receives the cache as `Some(Arc<PartialsCache>)` and re-supplies per-block partials when building each block's range proof.

`client/native::run_native_client` constructs `PartialExecutionWitness::from_preflight(partial, &exec)` when `args.kona.l1_head == 0xFF..FF` to assemble the witness on the fly (the full witness lives only briefly per partial proof).

**Rationale:** A single configuration knob (`--num-block-partials N`) selects the parallelism level; `0` preserves the existing monolithic path byte-for-byte; positive values trade additional aggregation overhead for parallel partial proving. The `PartialsCache` lets the aggregation pass deduplicate witness-side data: the same `PartialExecution` entries used for the partial proofs are re-used (by reference) for the aggregation proof's `Witness.partial_executions`.

## Risks / Trade-offs

**[Risk] Live DB reads during prestate authentication.** `CachedEvm::transact_raw` performs one `Database::basic` + N `Database::storage` calls per served transaction, plus a once-per-partial expected-state snapshot read. These reads happen during the aggregation proof and incur oracle round-trips. **Mitigation:** the cost is equivalent to the first access in a live-execution path (the state would have been read anyway when the tx executed); the pre-state reads warm the same `State.cache` entries a fall-through execution would have warmed. The expected-state check happens once per partial (gated by `expected_state_verified`), bounded by the fixed slot set.

**[Risk] Witness-supplied `original_info` must match the live aggregation DB.** If a partial was proven against one history and then replayed during aggregation on a divergent one, the per-tx prestate check rejects it. **Mitigation:** desired behavior — a wrong-history partial is exactly the attack vector the check closes. Operationally, the host produces partial witnesses from the same preflight pass that produces the aggregation-side traces, so divergence only happens if the witness was tampered with.

**[Risk] `ExpectedState` snapshot must cover every spec-required slot.** If a future OP hardfork adds a new fee-related L1Block slot, `EXPECTED_STORAGE_SLOTS` MUST be extended to include it — otherwise the partial can be tampered with to commit to a stale value of that slot and the divergence wouldn't be caught. **Mitigation:** the slot list is a single `const` array next to the OP-revm `constants::*_SLOT` re-exports it pulls from. A regression here would surface as a partial-roundtrip test failure (the host capture and guest re-derive would both miss the new slot identically — but a hostile witness could ship arbitrary values for it without being detected). Future-work: add a CI assertion that `EXPECTED_STORAGE_SLOTS` matches the slots the OP fee path reads.

**[Risk] Empty-partials path is the load-bearing baseline.** Because aggregation reuses the same code path as plain execution (Decision 13), a subtle bug in `CachedEvm` for empty caches would silently break the unmodified path. **Mitigation:** the pass-through behavior is covered by the pre-existing full derivation test suite (every existing test runs with empty partials), plus the `test_op_sepolia_*_partials_roundtrip` tests exercise both the capture pass and the replay pass.

**[Trade-off] Extra `.clone()` per captured `ResultAndState`.** On the host (and during guest trace capture) we clone the full `ResultAndState` into the collector (then convert to `PartialResultAndState`, which sorts). In practice this is one allocation + one O(n log n) sort per tx body transaction — amortized cost is negligible against proof time.

**[Trade-off] Increased proof count.** A block with `N` partials produces `N + 1` proofs (N partials + 1 aggregation) instead of 1. Proof composition via `env::verify()` adds recursive verification overhead. This is acceptable because the partial proofs run in parallel, reducing wall-clock time.

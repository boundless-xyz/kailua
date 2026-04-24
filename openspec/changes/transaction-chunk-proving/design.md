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
- Full backward compatibility — disabled by default (empty `partial_executions` + `pe_witness = None`).

**Non-Goals:**
- Automatic partition sizing based on transaction cost estimation.
- State-access conflict analysis for optimal parallelism (partials are sequential slices of the tx body).
- Cross-block transaction partitioning (partials are always within a single block).
- Modifying the on-chain verification contracts or fault-proof game.

## Decisions

### Decision 1: One unified `CachedEvm` / `CachedEvmFactory` wrapper serves all roles

**Choice:** A single `Evm`-trait wrapper (`CachedEvm<E: Evm>`) replaces what earlier iterations split across four types (tracing evm + factory, chunking evm + factory). Depending on construction it serves as:

1. **Transparent pass-through** — empty partial cache, no collector → every method delegates to the inner EVM.
2. **Trace collector** — empty cache + shared `TransactionResultCollector` → every successful fall-through `transact_raw` appends `(keccak256(tx.enveloped_tx), ResultAndState)` to the last inner `Vec` of the collector.
3. **Cached serve** — populated `Vec<PartialExecution>` (reversed) → `transact_raw` matches the incoming tx hash against the top partial's next `tx_hashes.last()`. Match → authenticate prestate then return cached `ResultAndState`. Miss → fall through to the inner EVM (and, if a collector is attached, capture the fresh execution).

The `CachedEvmFactory` keeps per-block partials in a reversed `Arc<Mutex<Vec<Vec<PartialExecution>>>>` and pops one block's worth per `create_evm` call. It also pushes a fresh per-EVM slot onto the shared trace collector before each EVM creation.

**Alternatives considered:**
- *Separate `TracingOpEvm` and `ChunkingEvm`.* Earlier design. Rejected because the two share >90% of their logic (all eight `Evm` trait methods are either delegation or a single branching call); keeping them separate would cost a factory split, two test suites, and a feature-gated `prover` crate boundary for no architectural benefit.
- *Modify `build_block()` to accept a hook.* Rejected — modifies vendored code.
- *Inspector-based tracing.* Rejected — wrong granularity (instruction-level vs tx-level), higher overhead.

**Rationale:** The `EvmFactory` is a generic parameter throughout `KonaExecutor<P, H, Evm>` and `StatelessL2Builder<P, H, Evm>`. Replacing it requires only changing `CachedExecutor::new()` generics — the vendored generic plumbing carries our factory through unchanged. Unifying to one type halves the surface we need to maintain and collapses the "tracing" and "aggregation" code paths in `run_core_client` into a single executor construction.

### Decision 2: Partial proofs reuse `ProofJournal` with a sentinel `l1_head`

**Choice:** Partial proofs emit the standard 220-byte `ProofJournal` with `l1_head = B256::repeat_byte(0xFF)` and `agreed_l2_output_root == claimed_l2_output_root`.

| Field | Value |
|-------|-------|
| `payout_recipient` | Identical to block proof |
| `precondition_hash` | `digest(Precondition::default().partial(pe_trace))` |
| `l1_head` | `0xFF..FF` (partial sentinel) |
| `agreed_l2_output_root` | Current agreed L2 output root |
| `claimed_l2_output_root` | Same as agreed (partial does not advance L2 state) |
| `claimed_l2_block_number` | Parent block number |
| `config_hash` | Identical to block proof |
| `fpvm_image_id` | Identical to block proof |

**Rationale:** Reusing `ProofJournal` means partial receipts flow through the existing `load_stitching_journals()` / `verify_stitching_journal()` infrastructure with zero changes. `l1_head = 0xFF..FF` distinguishes partials from block proofs and execution-only proofs (`l1_head = 0x00..00`).

### Decision 3: New `partial_executions` field on `Precondition`

**Choice:** Add `pub partial_executions: B256` as a fifth field on `Precondition`, with a `.partial(hash)` builder method. The `Digestible` impl treats a non-zero `partial_executions` as a third exclusive mode (all other fields must be zero).

**Rationale:** A partial proof commits to a fundamentally different assumption than `execution_trace` (block-level artifacts) or `derivation_cache`/`derivation_trace` (derivation-pipeline state). Overloading an existing field would conflate modes. A dedicated field keeps the assertion logic in `Digestible::digest()` simple and makes intent self-documenting.

### Decision 4: `partial_executions` is a two-input SHA256 — not a seven-input hash chain

**Choice:** `compute_pe_trace(results_hash, block_ctx_hash) = SHA256(results_hash || block_ctx_hash)`, where:

- `results_hash` folds the per-transaction `(tx_hash, ResultAndState)` vec through a canonical encoding that includes each `EvmState` entry's `original_info` (pre-tx `AccountInfo`) AND `EvmStorageSlot.original_value` (pre-tx slot value).
- `block_ctx_hash` covers every `BlockEnv` field and every `OpBlockExecutionCtx` field.

Earlier drafts committed to seven inputs (`tx_hash`, `pre_db_hash`, `post_db_hash`, `pre_evm_hash`, `post_evm_hash`, `results_hash`, `block_ctx_hash`) and chained pre/post DB and EVM-accumulator hashes across partials. That chaining is no longer necessary because `original_info` / `original_value` are now folded into `results_hash`.

**Rationale:** Once the per-tx `ResultAndState` commits to its own pre-state view, the aggregation side can authenticate that pre-state by reading the live `TrieDB` at the moment the cached result is served (Decision 5). No witness-level continuity hashes are required — tampering with a partial's `results` or its pre-state view produces a different `results_hash`, which produces a different `precondition_hash`, which makes `env::verify` reject the swapped assumption on the aggregation side.

A bonus: removing the hash chain eliminates `hash_cache`, `hash_cache_state`, `hash_overlay_state`, `EvmAccumulatorState`, `hash_evm_state`, `verify_block_chunks`, `PanicDB`, and the rkyv mirror types (`SerializableCache` / `SerializableDbAccount` / etc.) along with all their round-trip tests — a substantial subtraction from the maintenance surface.

### Decision 5: Per-transaction prestate authentication inside `CachedEvm::transact_raw`

**Choice:** When `CachedEvm::transact_raw` serves a cached `ResultAndState`, BEFORE returning it the wrapper:

1. Asserts `chunk.block_env == self.evm.block()` (rejects forged block context against the aggregation-side live env).
2. For every `(addr, account)` in the cached `state`:
    - Calls `Database::basic(db, addr)`. Asserts `stored_info == *account.original_info` (or requires `Created` / `LoadedAsNotExisting` status when `None`).
    - For every `(slot, evm_slot)`, calls `Database::storage(db, addr, slot)`. Asserts `actual == evm_slot.original_value`.

The `db` is the same `State<TrieDB>` the aggregation path commits final results to, so the reads resolve to the REAL L2 state. Any divergence — wrong account pre-state, wrong slot pre-value, stale `block_env` — panics before the cached result is handed to the block executor.

**Rationale:** This is the single load-bearing security decision. It replaces three earlier mechanisms — pre/post DB hash continuity, pre/post EVM-accumulator continuity, and a standalone `verify_block_chunks` header cross-check — with one tightly scoped in-EVM check. Because the authentication happens at the exact point the result flows into the rest of the aggregation (via `OpBlockExecutor`'s standard commit path), it is impossible to smuggle mismatched state through the boundary.

### Decision 6: Partial guests run against a real `State<TrieDB>` seeded with a `CacheState` prestate

**Choice:** The PARTIAL EXECUTION branch builds its state via `State::builder().with_database(TrieDB::new(...)).with_cached_prestate(cache).build()` — where `cache: CacheState` comes from the witness. Transactions are dispatched through the standard `OpBlockExecutor::execute_transaction`. There is no `PanicDB`, no accumulator seeding, no skipped pre-execution / finish, no hand-rolled tx loop.

**Alternatives considered:**
- *`CacheDB<PanicDB>`* — Earlier design. Rejected because the partial guest is already supplying a `CacheState` (which is structurally what `State<DB>` uses internally) and the `TrieDB` backing the live state is already available via the oracle. Panicking on missing state was a defensive measure that is now unnecessary — any miss falls through to the trie, which the oracle serves, and the resulting proof is still bound by `results_hash`.
- *Hand-rolled `execute_transaction_without_commit` + `commit_transaction` loop.* Rejected. The standard `execute_transaction` path automatically handles per-tx validation (block-available gas, Jovian DA footprint, L1Block preload), receipt building, deposit-nonce handling, and the commit step. Reproducing those rules is a maintenance tax every OP hardfork compounds.

**Rationale:** Simpler is safer: the partial guest exercises the exact same vendored execution path the monolithic and aggregation proofs use. Upstream OP hardfork changes (Isthmus, Interop, future forks) are inherited for free.

### Decision 7: `PartialExecution` carries only what cannot be derived at verification time

**Choice:** Four fields — `tx_hashes`, `results`, `block_env`, `op_block_ctx`. No `tx_count` (equals `tx_hashes.len()`), no `agreed_db` / `claimed_db` / `agreed_evm` / `claimed_evm` / `evm_state` (the per-tx prestate check in `CachedEvm` provides continuity at aggregation time, not a witness-level hash chain), no `block_number` / `chunk_index` (position in the outer vec is the block number offset, and within a block partials are consumed top-to-bottom by `CachedEvm`).

**Rationale:** Each omitted field was removable because `CachedEvm`'s serve-time authentication subsumed its role. The minimal struct keeps witnesses compact and the invariants obvious.

### Decision 8: `PartialExecutionWitness` needs no `EvmAccumulatorState`, no block metadata

**Choice:** Four fields — `transactions`, `cache: CacheState`, `block_env`, `op_block_ctx`. `PartialExecutionWitness::from_preflight(partial, execution)` builds the `CacheState` by folding `partial.results` through `cache_results(...)`, which lifts each account's `original_info` + initial-load storage into a `CacheState` and lifts inline bytecode into `cache.contracts` keyed by `code_hash`.

**Rationale:** The partial guest seeds the cache as a `State` prestate layer, runs transactions through the standard `OpBlockExecutor`, and captures its own trace. It does not need pre-computed accumulator seeds, because it is not continuing a chain — it is proving one bounded subsequence in isolation.

### Decision 9: `hash_results` canonical encoding binds `original_info`, ignores transient fields

**Choice:** The canonical encoding sorts accounts by `Address`, sorts storage slots by `U256`, and for each account encodes:

- Pre-tx `AccountInfo` fields: `nonce`, `balance`, `code_hash` (the `original_info` box).
- Post-tx `AccountInfo` fields: `nonce`, `balance`, `code_hash` (the live `info`).
- `AccountStatus` bitflags.
- Per-slot: `slot`, `original_value`, `present_value`.

Transient fields are explicitly excluded: `transaction_id`, `is_cold`, `AccountInfo.code` (the bytecode field; `code_hash` already commits to contents), `Account.original_info.code` (same reason).

**Rationale:** Including `original_info` is what makes the per-tx prestate authentication cryptographically binding — Decision 5 asserts that the live DB matches `original_info`, but only because `original_info` is itself authenticated through `results_hash` → `partial_executions` → journal identity. Excluding transient fields ensures the same logical trace hashes identically regardless of revm's internal execution context.

### Decision 10: Partial stitching is split between `precompute_pe_boots` and `stitch_partial_executions`

**Choice:** Before `run_core_client` consumes the `partial_executions` Vec, `run_stitching_client` calls `precompute_pe_boots(&partial_executions)` to produce `Vec<(precondition_hash, StitchedBootInfo)>` while the partial data is still borrowed. After `run_core_client` returns, `stitch_partial_executions(&boot, fpvm_image_id, payout_recipient, pe_boots, &proven_fpvm_journals)` reconstructs each partial's `ProofJournal` and verifies it against `proven_fpvm_journals`.

**Rationale:** The two-phase split allows `run_core_client` to consume the partials vector (moving it into `CachedEvmFactory`) while still allowing the stitching step to enumerate what was consumed. It also mirrors the existing `stitch_executions` pattern (journal reconstruction + `verify_stitching_journal` call).

### Decision 11: Unified `run_core_client` path — aggregation = execution with populated partials

**Choice:** Both the `EXECUTION ONLY` and `DERIVATION & EXECUTION` branches construct their executor with `CachedEvmFactory::new_with_traces(partial_executions, partials_collector)`. Empty `partial_executions` = transparent pass-through; populated = cached serve with inline prestate authentication. There is no separate "aggregation mode" and no branching on chunk presence.

**Rationale:** Collapsing the two modes into one code path removes an entire class of branching logic — every test we have for "partial path" also exercises "non-partial path" when the vec is empty. This is what makes the `partial_executions = Vec::new()` default a safe, byte-identical no-op.

### Decision 12: `EvmFactory`-generic `CachedExecutor`

**Choice:** `CachedExecutor::new` takes a generic `Evm: EvmFactory<Spec = OpSpecId, BlockEnv = BlockEnv> + Send + Sync + Clone + Debug + 'static` parameter. The factory flows through to `KonaExecutor::new`. All call sites pass a `CachedEvmFactory`.

**Rationale:** The refactor is purely mechanical — the pre-refactor specialized impl block simply becomes one point in the generic space. It unlocks Decision 1 (one `Evm` wrapper serving multiple roles) without duplicating the executor wiring.

## Risks / Trade-offs

**[Risk] Live DB reads during prestate authentication.** `CachedEvm::transact_raw` performs one `Database::basic` + N `Database::storage` calls per served transaction. These reads happen during the aggregation proof and incur oracle round-trips. **Mitigation:** the cost is equivalent to the first access in a live-execution path (the state would have been read anyway when the tx executed); the pre-state reads warm the same `State.cache` entries a fall-through execution would have warmed.

**[Risk] Witness-supplied `original_info` must match the live aggregation DB.** If a partial was proven against one history and then replayed during aggregation on a divergent one, the per-tx prestate check rejects it. **Mitigation:** desired behavior — a wrong-history partial is exactly the attack vector the check closes. Operationally, the host produces partial witnesses from the same preflight pass that produces the aggregation-side traces, so divergence only happens if the witness was tampered with.

**[Risk] Empty-partials path is the load-bearing baseline.** Because aggregation reuses the same code path as plain execution (Decision 11), a subtle bug in `CachedEvm` for empty caches would silently break the unmodified path. **Mitigation:** the pass-through behavior is covered by the pre-existing full derivation test suite (every existing test runs with empty partials), plus the `test_op_sepolia_*_partials_roundtrip` tests exercise both the capture pass and the replay pass.

**[Trade-off] Extra `.clone()` per captured `ResultAndState`.** On the host (and during guest trace capture) we clone the full `ResultAndState` into the collector. In practice this is one allocation per tx body transaction — amortized cost is negligible against proof time.

**[Trade-off] Increased proof count.** A block with `N` partials produces `N + 1` proofs (N partials + 1 aggregation) instead of 1. Proof composition via `env::verify()` adds recursive verification overhead. This is acceptable because the partial proofs run in parallel, reducing wall-clock time.

## ADDED Requirements

### Requirement: `PartialExecution` struct carries a proven transaction subsequence

The system SHALL define a `PartialExecution` struct in `kailua-kona` at `crates/kona/src/evm.rs` representing a proven subsequence of a block's ordered transaction body. Fields:

- `tx_hashes: Vec<B256>` — EIP-2718 `keccak256` hashes of each transaction in this subsequence, in execution order.
- `results: Vec<ResultAndState<OpHaltReason>>` — full per-transaction `ExecutionResult` + `EvmState` captured during proof execution, in execution order. `results.len() == tx_hashes.len()`.
- `block_env: BlockEnv` — the `BlockEnv` under which the partial guest executed these transactions.
- `op_block_ctx: OpBlockExecutionCtx` — the OP-specific block execution context.

A method `precondition_hash(&self) -> B256` SHALL return `compute_pe_trace(hash_results(&tx_hashes, &results), hash_block_ctx(&block_env, &op_block_ctx))`.

Notably: there are **no** pre/post memory-DB hashes, pre/post EVM-accumulator hashes, or explicit `tx_count` field. Continuity across partials within the same block is enforced at aggregation time by `CachedEvm`'s per-tx prestate authentication, not by witness-level hash chains.

#### Scenario: results and tx_hashes are parallel vectors
- **WHEN** a `PartialExecution` is instantiated
- **THEN** `results.len() == tx_hashes.len()`

#### Scenario: precondition_hash is the two-input `compute_pe_trace`
- **WHEN** `PartialExecution::precondition_hash()` is called
- **THEN** it returns `compute_pe_trace(hash_results(tx_hashes, results), hash_block_ctx(block_env, op_block_ctx))`

### Requirement: `CachedEvm` serves cached `ResultAndState` entries and authenticates their prestate against the live DB

The system SHALL provide a `CachedEvm<E: Evm>` struct in `kailua-kona` at `crates/kona/src/evm.rs` wrapping any `E: Evm<HaltReason = OpHaltReason, Tx = OpTransaction<TxEnv>>`. Its instance state holds a reversed `Vec<PartialExecution>` (so `pop()` yields the next partial in execution order) and an optional shared `TransactionResultCollector`. On construction each partial's inner `tx_hashes` and `results` vectors are ALSO reversed in parallel, so `pop()` yields the next `(tx_hash, ResultAndState)` pair.

On `transact_raw(tx)`:

1. Compute `incoming_hash = keccak256(tx.enveloped_tx)`. The `enveloped_tx` field MUST be present.
2. Peel off any fully-consumed partials from the top of the cache.
3. If the top partial's next `tx_hashes.last()` equals `incoming_hash`:
    - Assert `chunk.block_env == self.evm.block()` — a forged partial `block_env` is caught here.
    - For every `(addr, account)` in the cached `ResultAndState.state`:
        - Call `Database::basic(db, addr)`. Assert `stored_info == account.original_info`. (When `actual_info.is_none()`, the account MUST be marked `Created` or `LoadedAsNotExisting`.)
        - For every `(slot, evm_slot)` in the account's storage, call `Database::storage(db, addr, slot)`. Assert `actual == evm_slot.original_value`.
    - Pop the cached `(tx_hash, ResultAndState)` pair and return it.
4. Otherwise, delegate to `self.evm.transact_raw(tx)`. On `Ok`, if a `collection_target` is attached, append `(incoming_hash, ResultAndState)` to the last inner `Vec` of that collector.

`transact_system_call` SHALL always delegate to the inner EVM. All other `Evm` trait methods (`block`, `chain_id`, `finish`, `set_inspector_enabled`, `components`, `components_mut`) SHALL delegate to the inner EVM.

This per-tx prestate authentication is what makes the entire pre/post-hash chain from earlier designs unnecessary: a witness-supplied `ResultAndState` is only accepted if its `original_info` / `original_value` view agrees with the live DB the aggregation path is running against. Any divergence (wrong account pre-state, wrong slot pre-value, wrong block_env) aborts the aggregation proof.

#### Scenario: matching tx hash serves the cached result
- **WHEN** `CachedEvm::transact_raw(tx)` is called and the top partial's next `tx_hashes.last()` matches `keccak256(tx.enveloped_tx)`
- **THEN** the cached `ResultAndState` is returned without invoking `self.evm.transact_raw(tx)`

#### Scenario: mismatching tx hash falls through to the inner EVM
- **WHEN** no partial is active, or the top partial's next `tx_hashes.last()` does not match `keccak256(tx.enveloped_tx)`
- **THEN** the call delegates to `self.evm.transact_raw(tx)` and, if a collector is attached, the fresh `(tx_hash, ResultAndState)` is appended to the collector's last inner `Vec`

#### Scenario: account prestate mismatch aborts
- **WHEN** serving a cached entry finds `Database::basic(db, addr) != *account.original_info`
- **THEN** `CachedEvm::transact_raw` panics with a "account prestate mismatch" message

#### Scenario: storage prestate mismatch aborts
- **WHEN** serving a cached entry finds `Database::storage(db, addr, slot) != evm_slot.original_value`
- **THEN** `CachedEvm::transact_raw` panics with a "storage prestate mismatch" message

#### Scenario: forged block_env aborts
- **WHEN** the cached partial's `block_env` differs from the inner EVM's `block()`
- **THEN** `CachedEvm::transact_raw` panics with "BlockEnv mismatch"

#### Scenario: `transact_system_call` does not consume cached entries
- **WHEN** the block executor issues prelude / epilogue system calls
- **THEN** those calls delegate to the inner EVM and never advance the partial cursor or append to the trace collector

### Requirement: `CachedEvmFactory` produces per-block `CachedEvm` instances positionally

The system SHALL provide a `CachedEvmFactory` (in `kailua-kona`) that holds an `Arc<Mutex<Vec<Vec<PartialExecution>>>>` populated in reverse execution order and an optional shared `TransactionResultCollector`. It SHALL implement `EvmFactory` with `type Evm<DB, I> = CachedEvm<OpEvm<DB, I, PrecompilesMap>>`. `create_evm` and `create_evm_with_inspector` SHALL:

1. Pop the next block's `Vec<PartialExecution>` (empty if exhausted) via `take_next_chunks`.
2. Push an empty slot onto the shared trace collector (`push_trace_slot`), so the new `CachedEvm`'s captures land in a fresh per-EVM `Vec`.
3. Delegate to the inner `OpEvmFactory` to construct an `OpEvm`, then wrap in `CachedEvm::new_with_traces(inner, popped_partials, block_traces)`.

The factory is consumed positionally: the outer index in the caller-supplied `Vec<Vec<PartialExecution>>` corresponds to the Nth L2 block the factory is used for (during normal derivation this is ascending block order from `safe_head_number + 1`). `take_all_block_traces()` returns an `Arc`-safe drained snapshot of per-block captured traces, one inner `Vec` per EVM the factory created.

An empty `Vec<Vec<PartialExecution>>` (or a block position whose inner vec is empty) is equivalent to using `OpEvmFactory` directly: every `transact_raw` falls through to the inner EVM.

#### Scenario: factory serves positional block chunks
- **WHEN** a caller supplies `[[p_block1_a, p_block1_b], [], [p_block3_a]]` and calls `create_evm` three times
- **THEN** the first call's `CachedEvm` is seeded with `[p_block1_a, p_block1_b]`, the second with `[]` (empty), and the third with `[p_block3_a]`

#### Scenario: empty outer vec is a no-op wrapper
- **WHEN** `CachedEvmFactory::new(Vec::new())` is used to build every EVM in a run
- **THEN** each produced `CachedEvm` delegates every `transact_raw` to the inner `OpEvm`, producing the exact same outcome as `OpEvmFactory::default()`

#### Scenario: trace collection is per-EVM
- **WHEN** a factory has a collector attached and two consecutive EVMs each execute transactions
- **THEN** `take_all_block_traces()` returns two inner `Vec`s — one per EVM — each containing only that EVM's captured `(tx_hash, ResultAndState)` pairs

### Requirement: `run_core_client` always routes through `CachedEvmFactory`

Both the `EXECUTION ONLY` and `DERIVATION & EXECUTION` branches of `run_core_client` (in `kailua-kona` at `crates/kona/src/client/core.rs`) SHALL construct their executor with `CachedEvmFactory::new_with_traces(partial_executions, partials_collector)` rather than `OpEvmFactory::default()`. This unifies the code path — when `partial_executions` is empty, `CachedEvm` degrades to a transparent `OpEvm` wrapper; when populated, it serves cached results and authenticates prestate inline. There is no separate "aggregation mode" code path: chunk aggregation is transparent at the executor level and happens entirely inside the EVM layer.

An optional `partials_collector: Option<TransactionResultCollector>` parameter SHALL let callers capture `(tx_hash, ResultAndState)` pairs for every non-cached execution — used by the host to assemble `PartialExecution` entries via `recover_collected_partials` after `run_core_client` returns.

#### Scenario: empty partials preserves baseline behavior
- **WHEN** `run_core_client` is called with `partial_executions = Vec::new()`
- **THEN** block execution uses a `CachedEvmFactory` whose per-block partials are always empty, so every `transact_raw` delegates to the inner `OpEvm` — producing byte-identical output to the pre-partial-executions code path

#### Scenario: populated partials serve cached results
- **WHEN** `run_core_client` is called with `partial_executions` whose outer indices align with the range of blocks being produced
- **THEN** each block's transactions whose hash appears in the positional partial are served from the cache (with per-tx prestate authentication); transactions not in any partial fall through to the inner EVM

### Requirement: Partial proofs are stitched via `precompute_pe_boots` + `stitch_partial_executions`

Partial proof receipts SHALL be verified in `run_stitching_client()` (in `kailua-kona` at `crates/kona/src/client/stitching.rs`) alongside `stitch_executions()` and `stitch_boot_info()`. Two functions cooperate:

1. `precompute_pe_boots(partial_executions: &[Vec<PartialExecution>]) -> Vec<(B256, StitchedBootInfo)>` — Called BEFORE `run_core_client` consumes the partials (so their fields are still accessible). For each partial in each block, it computes `precondition_hash = partial.precondition_hash()` and assembles a `StitchedBootInfo { l1_head: 0xFF..FF, agreed_l2_output_root: partial.op_block_ctx.parent_hash, claimed_l2_output_root: partial.op_block_ctx.parent_hash, claimed_l2_block_number: partial.block_env.number - 1 }`.

2. `stitch_partial_executions(boot, fpvm_image_id, payout_recipient, pe_boots, &proven_fpvm_journals)` — Called AFTER `run_core_client` returns. For each precomputed `(precondition_hash, stitched_boot)` it constructs `ProofJournal::new_stitched(fpvm_image_id, payout_recipient_address, precondition_hash, config_hash, &stitched_boot).encode_packed()` and calls `verify_stitching_journal(fpvm_image_id, encoded_journal, &proven_fpvm_journals)`.

The aggregation side does NOT perform a standalone `verify_block_chunks` cross-check against the derivation-produced header: the `block_env` / `op_block_ctx` cross-check lives inline in `CachedEvm::transact_raw` (via the `block_env` assertion and per-tx prestate authentication), and `hash_block_ctx` is folded into each partial's `precondition_hash` so the `stitch_partial_executions` journal reconstruction fails for any forged context.

#### Scenario: stitching runs alongside execution stitching
- **WHEN** `run_stitching_client()` completes
- **THEN** it invokes `stitch_partial_executions` and `stitch_executions` (and then `stitch_boot_info`) using the same `proven_fpvm_journals` set loaded once at the top

#### Scenario: partials `StitchedBootInfo` pins the parent block
- **WHEN** a partial belongs to block N
- **THEN** the reconstructed `StitchedBootInfo` has `agreed_l2_output_root == claimed_l2_output_root == partial.op_block_ctx.parent_hash` and `claimed_l2_block_number == N - 1`

#### Scenario: forged partial contents fail `env::verify`
- **WHEN** a witness ships a partial whose `results` / `tx_hashes` / `block_env` / `op_block_ctx` disagree with what the partial guest actually produced
- **THEN** `stitch_partial_executions` computes a `precondition_hash` different from the one folded into the proven partial journal, so the reconstructed `ProofJournal` does not appear in `proven_fpvm_journals` and `verify_stitching_journal` rejects the assumption

### Requirement: Trie root provides ultimate correctness guarantee

The aggregation proof SHALL rely on two complementary verification layers:

1. **Per-tx prestate authentication in `CachedEvm`** — every cached `ResultAndState` is bound to the live aggregation DB before being returned to the block executor.
2. **Trie root computation in the standard `build_block` pipeline** — served and fallen-through `ResultAndState` entries are committed to `State<TrieDB>` through the upstream `OpBlockExecutor` → `finish` → `merge_transitions` → `seal_block` path. `state_root = trie_db.state_root(&bundle)` is computed inside the proof, and any incorrect chunk results produce a different root — which fails the block-level output-root check.

The aggregation proof does NOT recompute or compare flat-cache hashes: the per-tx prestate check authenticates the ingress side; the trie root authenticates the egress side.

#### Scenario: wrong chunk results produce wrong trie root
- **WHEN** a partial's `ResultAndState.state` does not match what live execution would produce
- **THEN** the state root computed by `State<TrieDB>` differs from the correct value and the block-level output root does not match the claimed root

#### Scenario: joint correctness
- **WHEN** every partial's prestate check passes AND the final state root matches the claimed L2 output root
- **THEN** the aggregated block proof is valid and the state transition is correct

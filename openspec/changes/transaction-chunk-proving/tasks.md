## 1. Precondition: `partial_executions` field

- [x] 1.1 Add `pub partial_executions: B256` field to `Precondition` with `#[rkyv(with = B256Def)]`. Add `.partial(trace: B256) -> Self` builder. Ensure `Default` still produces all-zero fields.
- [x] 1.2 Extend `Digestible` to dispatch the new field as a third exclusive mode: when `partial_executions` is non-zero, assert every other field is zero and return `Digest::from_bytes(partial_executions.0)`.
- [x] 1.3 Unit tests: default-is-all-zero, builder sets only the partial field, partial-only digest, partial + each other non-zero field panics, execution-only and derivation/proposal branches unchanged.
- [x] 1.4 Run `kailua-kona` tests — zero regressions.

## 2. `compute_pe_trace`, `hash_results`, `hash_block_ctx` (two-input partial commitment)

- [x] 2.1 Create `crates/kona/src/precondition/evm.rs` alongside the existing `execution.rs`, `proposal.rs`, `derivation.rs`. Define `compute_pe_trace(results_hash, block_ctx_hash) -> B256 = SHA256(results_hash || block_ctx_hash)`.
- [x] 2.2 Define `hash_block_ctx(block_env, op_block_ctx) -> B256` with canonical flatteners: `flatten_block_env` (number / beneficiary / timestamp / gas_limit / basefee / difficulty / option<prevrandao> / option<(excess_blob_gas, blob_gasprice)>) and `flatten_op_block_execution_ctx` (parent_hash / option<parent_beacon_block_root> / length-prefixed extra_data).
- [x] 2.3 Define `hash_results(tx_hashes, results) -> B256` with canonical encoding: per-tx block is `tx_hash || flatten_execution_result(result) || flatten_evm_state(state)`. `flatten_execution_result` covers Success (reason discriminant, gas_used, gas_refunded, logs, output), Revert (gas_used, output), Halt (OpHaltReason discriminants, gas_used). `flatten_evm_state` sorts accounts by address and slots by key; each account encodes `original_info` (nonce/balance/code_hash) + live `info` + `status.bits()` + sorted storage (slot, original_value, present_value). Transient fields excluded: `transaction_id`, `is_cold`, `AccountInfo.code`.
- [x] 2.4 Unit tests: deterministic, input-sensitive, order-sensitive, `original_info` contributes, transient fields do not, Success/Revert/Halt variants hash differently, empty-trace has a well-defined non-zero hash. Integration test: `Precondition::default().partial(compute_pe_trace(a, b)).digest() == Digest::from_bytes(pe_trace.0)`.
- [x] 2.5 Run `kailua-kona` tests — zero regressions.

## 3. Unified `CachedEvm` / `CachedEvmFactory`

- [x] 3.1 Create `crates/kona/src/evm.rs`. Define `CachedEvm<E: Evm>` with `cache: Vec<PartialExecution>` (reversed at construction), `evm: E`, `collection_target: Option<TransactionResultCollector>`. Constructor `new_with_traces(evm, cache, collection_target)` reverses `cache`, and each `chunk.tx_hashes` / `chunk.results` pair in parallel, so `pop()` yields execution-order entries.
- [x] 3.2 Implement `Evm` for `CachedEvm<E>` with `E: Evm<HaltReason = OpHaltReason, Tx = OpTransaction<TxEnv>>` and `E::DB: Database`. Delegate `block`, `chain_id`, `finish`, `set_inspector_enabled`, `components`, `components_mut`, `transact_system_call` to the inner EVM. `transact_raw` computes `incoming_hash = keccak256(tx.enveloped_tx)`, peels exhausted partials, and either serves-with-authentication or falls-through-with-capture.
- [x] 3.3 Serve path: assert `chunk.block_env == self.evm.block()`; for every `(addr, account)` in the cached state, call `Database::basic(db, addr)` and assert against `account.original_info` (or require `Created` / `LoadedAsNotExisting` on `None`); for every `(slot, evm_slot)`, call `Database::storage(db, addr, slot)` and assert against `original_value`.
- [x] 3.4 Fall-through path: delegate to `self.evm.transact_raw(tx)`; on `Ok`, if a collector is attached, append `(incoming_hash, result.clone())` to the collector's last inner `Vec`.
- [x] 3.5 Define `CachedEvmFactory` holding `inner: OpEvmFactory`, `cache: Arc<Mutex<Vec<Vec<PartialExecution>>>>` (reversed at construction), `block_traces: Option<TransactionResultCollector>`. Constructors `new(cache)` and `new_with_traces(cache, block_traces)`. `take_next_chunks` pops the next block's partials; `push_trace_slot` appends an empty `Vec` to the collector; `take_all_block_traces` atomically drains the collector.
- [x] 3.6 Implement `EvmFactory` for `CachedEvmFactory` with `type Evm<DB, I> = CachedEvm<OpEvm<DB, I, PrecompilesMap>>` and OP-specific associated types. Both `create_evm` and `create_evm_with_inspector` take the next block's partials, push a fresh trace slot, delegate to `inner`, and wrap via `CachedEvm::new_with_traces`.
- [x] 3.7 Unit tests (`crates/kona/src/evm.rs::tests`): pre-computed results returned in order (marker gas_used), system calls delegate, empty cache delegates every `transact_raw`, multi-partial cursor crosses boundaries, factory routes per block, `take_next_chunks` is destructive, exhausted cache delegates, prestate authentication panics on account mismatch / storage mismatch / forged block_env.
- [x] 3.8 Run `kailua-kona` tests — zero regressions.

## 4. `CachedExecutor` generic over `EvmFactory`

- [x] 4.1 Generalize `impl CachedExecutor<KonaExecutor<'a, P, H, OpEvmFactory>>` to `impl<Evm: EvmFactory<Spec = OpSpecId, BlockEnv = BlockEnv> + Send + Sync + Clone + Debug + 'static> CachedExecutor<KonaExecutor<'a, P, H, Evm>>`. Pass the factory through to `KonaExecutor::new`.
- [x] 4.2 Update every call site in `crates/kona/src/client/core.rs` and elsewhere to pass `CachedEvmFactory::new_with_traces(partial_executions, partials_collector)`.
- [x] 4.3 Run pre-existing executor tests — identical behavior when partials are empty and no collector is attached.

## 5. `PartialExecution`, `PartialExecutionWitness`, host witness assembly

- [x] 5.1 Define `PartialExecution` in `crates/kona/src/evm.rs` with fields `tx_hashes`, `results`, `block_env`, `op_block_ctx` and their rkyv `ArchiveWith` wrappers in `crates/kona/src/rkyv/evm.rs` (`BlockEnvRkyv`, `OpBlockExecutionCtxRkyv`, `ResultAndStateRkyv`, `AccountRkyv`, `EvmStateRkyv`, `ExecutionResultRkyv`, halt-reason helpers). Implement `PartialExecution::precondition_hash()` returning `compute_pe_trace(hash_results(&tx_hashes, &results), hash_block_ctx(&block_env, &op_block_ctx))`.
- [x] 5.2 Define `PartialExecutionWitness` in `crates/kona/src/evm.rs` with `transactions: Vec<Vec<u8>>`, `cache: CacheState` (serialized via `CacheStateRkyv`), `block_env`, `op_block_ctx`. Define `cache_results(results) -> CacheState` that folds a `Vec<ResultAndState>` into a `CacheState` — inserting each address's `original_info` + first-load storage, and lifting inline bytecode into `cache.contracts` keyed by `code_hash`.
- [x] 5.3 Define `PartialExecutionWitness::new(partial, transactions)` and `PartialExecutionWitness::from_preflight(partial, execution)`, the latter looking up transaction bytes via `execution.get_transactions(&partial.tx_hashes)`.
- [x] 5.4 Add `pub fn get_transactions(&self, tx_hashes: &[B256]) -> Vec<Bytes>` on `Execution` (in `crates/kona/src/executor.rs`) — builds a per-call `HashMap<B256, Bytes>` from the attributes' tx list and looks up each requested hash.
- [x] 5.5 Define `build_single_partial_for_block(execution, traces, parent_header, spec_id)` in `crates/kona/src/executor.rs` — unzips `(tx_hashes, results)`, reconstructs `BlockEnv` from the sealed header (using `expected_blob_excess_gas_and_price(parent_header, spec_id)` for the blob pricing field), and fills `OpBlockExecutionCtx` from `parent_hash` / `parent_beacon_block_root` / `extra_data`.
- [x] 5.6 Define `recover_collected_partials(boot_info, collector, execution_cache, parent_header)` in `crates/kona/src/client/core.rs`. Iterates the drained collector lock-step with the `Execution` cache, emits one `PartialExecution` per block (full-block range), threads `parent_header` through the loop.
- [x] 5.7 Unit / integration tests: `test_op_sepolia_16491249_16491250_partials_roundtrip`, `test_op_sepolia_16491249_16491349_partials_roundtrip`, `test_op_sepolia_16491249_16491250_split_partials_roundtrip`, `test_op_sepolia_16491249_16491349_partials_roundtrip` (per-partial `test_partial` invocation + post-pass replay cross-check).
- [x] 5.8 Run `kailua-kona` tests — zero regressions.

## 6. `validate_cache` (contract-bytecode authentication)

- [x] 6.1 Define `validate_contract_hash(expected: &B256, bytecode: &Bytecode)` and `validate_cache(cache: &CacheState)` in `crates/kona/src/client/core.rs`. The top-level `cache.contracts` map AND every inline `PlainAccount.info.code` entry are validated against their `code_hash` key via `bytecode.hash_slow() == code_hash`. The zero-hash case requires an empty bytecode.
- [x] 6.2 Unit tests: valid pair passes, modified bytecode without updated key fails, different validated pair changes the resulting hash.
- [x] 6.3 Run `kailua-kona` tests — zero regressions.

## 7. PARTIAL EXECUTION branch in `run_core_client`

- [x] 7.1 Add `pe_witness: Option<PartialExecutionWitness>` field to `Witness`, threaded through `run_stateless_client` → `run_stitching_client` → `run_core_client` as an explicit parameter.
- [x] 7.2 Add a `partials_collector: Option<TransactionResultCollector>` parameter on `run_core_client` for host-side capture. Populate from tests; leave `None` in guest code paths.
- [x] 7.3 Implement the PARTIAL EXECUTION branch when `boot.l1_head == B256::repeat_byte(0xFF)`:
    - Extract `PartialExecutionWitness { transactions, block_env, op_block_ctx, cache }` (error if `pe_witness == None`).
    - Compute `block_ctx_hash = hash_block_ctx(&block_env, &op_block_ctx)`.
    - Load the safe-head header via `OracleL2ChainProvider`; assert `agreed == claimed` and `claimed_l2_block_number == safe_head.number`.
    - Validate contracts via `validate_cache(&cache)`.
    - Build `State::builder().with_database(TrieDB::new(safe_head, l2_provider, l2_provider)).with_cached_prestate(cache).build()`.
    - Construct `CfgEnv` with the chain id and `spec_id(block_env.timestamp)`; construct `EvmEnv::new(cfg_env, block_env)`.
    - Construct `CachedEvmFactory::new_with_traces(Vec::new(), Some(Arc::new(Mutex::new(Vec::new()))))`, construct `OpBlockExecutor::new(evm, op_block_ctx, rollup_config, OpAlloyReceiptBuilder::default())`.
    - For each tx bytes: `OpTxEnvelope::decode_2718_exact` → `try_into_recovered` → `WithEncoded::new` → `op_block_executor.execute_transaction(...)`.
    - Drain `take_all_block_traces()`, unzip into `(captured_tx_hashes, captured_results)`, compute `results_hash = hash_results(&captured_tx_hashes, &captured_results)`, then `pe_trace = compute_pe_trace(results_hash, block_ctx_hash)`, and return `(boot, Precondition::default().partial(pe_trace))`.
- [x] 7.4 Integration tests: `test_partial` helper drives the PARTIAL branch directly with a hand-constructed `BootInfo { l1_head: 0xFF..FF, agreed == claimed, .. }` + `PartialExecutionWitness`. Verified that the returned `precondition` equals `Precondition::default().partial(partial_execution.precondition_hash())`.
- [x] 7.5 Run `kailua-kona` tests — zero regressions.

## 8. `partial_executions` vec, stitching, and unified aggregation

- [x] 8.1 Add `partial_executions: Vec<Vec<PartialExecution>>` field to `Witness`, threaded through `run_stateless_client` → `run_stitching_client` → `run_core_client` as an explicit parameter.
- [x] 8.2 In the EXECUTION ONLY and DERIVATION & EXECUTION branches of `run_core_client`, construct `CachedEvmFactory::new_with_traces(partial_executions, partials_collector)` and pass it to `CachedExecutor::new` / `KonaExecutor::new`.
- [x] 8.3 Extend the `StitchingClient` trait signature and `KonaStitchingClient` implementation to accept `pe_witness: Option<PartialExecutionWitness>` and `partial_executions: Vec<Vec<PartialExecution>>`. Update every call site (`kailua-hana`, `kailua-hokulea`, `kailua-prover::client::witgen`).
- [x] 8.4 Define `precompute_pe_boots(partial_executions: &[Vec<PartialExecution>]) -> Vec<(B256, StitchedBootInfo)>` in `crates/kona/src/client/stitching.rs`. For each partial: compute `precondition_hash = partial.precondition_hash()` and `StitchedBootInfo { l1_head: 0xFF..FF, agreed_l2_output_root: partial.op_block_ctx.parent_hash, claimed_l2_output_root: partial.op_block_ctx.parent_hash, claimed_l2_block_number: partial.block_env.number - 1 }`.
- [x] 8.5 Define `stitch_partial_executions(boot, fpvm_image_id, payout_recipient, pe_boots, &proven_fpvm_journals)` — constructs `ProofJournal::new_stitched(...)` from each precomputed entry and calls `verify_stitching_journal`.
- [x] 8.6 Call `precompute_pe_boots` from `run_stitching_client` BEFORE `run_core_client` (while partials are still borrowed), then call `stitch_partial_executions` AFTER `run_core_client` returns (alongside `stitch_executions` and `stitch_boot_info`).
- [x] 8.7 Unit tests: `precompute_pe_boots` produces empty output for empty input, one entry per partial for populated input with correct `agreed == claimed == parent_hash` and `claimed_l2_block_number == N - 1`.
- [x] 8.8 Integration tests (`test_op_sepolia_*_partials_roundtrip`): capture partials on pass 1, replay on pass 2 with the captured partials seeded. Verify pass-2 captures are empty (all served from cache) and block headers match pass 1.
- [x] 8.9 Run `kailua-kona` tests — zero regressions.

## 9. Prover-side dispatch *(not yet implemented)*

- [x] 9.0 `run_witgen_client` now accepts and threads `partial_executions: Vec<Vec<PartialExecution>>` into `run_core_client` and into `Witness.partial_executions`. All callers pass `Vec::new()` until 9.1-9.3 land.
- [ ] 9.1 Add `max_txs_per_chunk: usize` to `ProvingArgs` (default `usize::MAX`; reject `0`).
- [ ] 9.2 In `crates/prover/src/tasks.rs`, when `0 < max_txs_per_chunk < block_tx_count`, slice the captured full-block `PartialExecution` into sub-partials cloning the shared `block_env` / `op_block_ctx`, build a `PartialExecutionWitness::from_preflight(sub_partial, execution)` for each, and dispatch partial proof jobs in parallel via `seek_proof()`.
- [ ] 9.3 Collect partial receipts and assemble the aggregation witness: populate `Witness.partial_executions` at the block's outer index and append receipts to `stitched_proofs`.
- [ ] 9.4 Integration test: mock block, set `max_txs_per_chunk`, verify partials are constructed correctly and jobs are dispatched.
- [ ] 9.5 Integration test: `max_txs_per_chunk = usize::MAX` reproduces current monolithic behavior byte-for-byte.

## 10. End-to-end validation *(partially shipped)*

- [x] 10.1 End-to-end parity test: prove a known block via a single full-block partial AND via a `split_partials`-decomposed version (one-tx-per-partial). Verified that both produce identical aggregation output.
- [ ] 10.2 End-to-end test with 3+ partials where cross-partial state dependencies exist (later partial reads earlier partial's write, contract creation, selfdestruct). Verify aggregation produces a correct state root.
- [ ] 10.3 End-to-end test combining block-level stitching (`max_block_executions`) with transaction-level partial decomposition (`max_txs_per_chunk`).
- [ ] 10.4 Full workspace test-suite pass once 9.x lands; verify zero regressions across kailua-kona, kailua-prover, kailua-hana, kailua-hokulea.

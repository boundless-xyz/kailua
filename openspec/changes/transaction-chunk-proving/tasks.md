## 1. Precondition: chunk_trace field

- [ ] 1.1 Add `pub chunk_trace: B256` field to `Precondition` struct in `crates/kona/src/precondition/mod.rs` with `#[rkyv(with = B256Def)]` annotation. Add `.chunk(chunk_trace: B256) -> Self` builder method. Ensure `Default` still produces all-zero fields.
- [ ] 1.2 Extend `Digestible` implementation: when `chunk_trace` is non-zero, assert all other fields are zero, return `Digest::from_bytes(chunk_trace.0)`. Existing execution-only and derivation/proposal branches remain unchanged.
- [ ] 1.3 Add unit tests for `Precondition::digest()`: chunk-only mode, execution-only mode (unchanged), derivation/proposal mode (unchanged), mixed-field assertion panics (chunk_trace + execution_trace, chunk_trace + derivation_cache, etc.), default is all-zero.
- [ ] 1.4 Run existing `kailua-kona` test suite — verify zero regressions from the new field. Fix any rkyv serialization tests that depend on `Precondition` byte layout.

## 2. Memory DB canonical hashing

- [ ] 2.1 Create `crates/kona/src/hash/mod.rs` module with a `hash_cache(cache: &Cache) -> B256` function that computes deterministic SHA256 over revm's `Cache` struct. Canonical encoding: accounts (sorted by Address) with full AccountInfo + storage (sorted by U256) + AccountState, then contracts (sorted by code_hash) with bytecode, then block_hashes (sorted by U256 block number).
- [ ] 2.2 Add unit tests for `hash_cache`: empty cache, single account with no storage, single account with multiple storage slots (verify sort order independence), multiple accounts, contracts present, block_hashes present, modification of any field changes the hash, round-trip determinism (insert in different orders → same hash).
- [ ] 2.3 Create `hash_evm_state(evm_state: &EvmAccumulatorState) -> B256` function in the same module. Define `EvmAccumulatorState` struct with `cumulative_gas_used: u64`, `da_footprint_used: u64`, `blob_gas_used: u64`, `logs_bloom: Bloom`, `receipts: Vec<OpReceiptEnvelope>`. Hash with SHA256 over canonical encoding.
- [ ] 2.4 Add unit tests for `hash_evm_state`: initial (zeroed) state, state with receipts, modification of any field changes the hash, receipt ordering matters.
- [ ] 2.5 Create `compute_chunk_trace(tx_hash: B256, pre_db_hash: B256, post_db_hash: B256, pre_evm_hash: B256, post_evm_hash: B256) -> B256` function. Returns `SHA256(tx_hash || pre_db_hash || post_db_hash || pre_evm_hash || post_evm_hash)`.
- [ ] 2.6 Add unit tests for `compute_chunk_trace`: determinism, any input change produces different output, integration with `Precondition::default().chunk(trace).digest()`.
- [ ] 2.7 Run full `kailua-kona` test suite — verify zero regressions.

## 3. TracingOpEvm and TracingEvmFactory

- [ ] 3.1 Create `crates/kona/src/tracing_evm.rs`. Define `TracingOpEvm<DB, I>` struct wrapping `OpEvm<DB, I, PrecompilesMap>` with `traces: Arc<Mutex<Vec<EvmState>>>`. Implement all 8 `Evm` trait methods: `block()`, `chain_id()`, `finish()`, `set_inspector_enabled()`, `components()`, `components_mut()` delegate directly. `transact_raw()` delegates then clones `result.state` into traces on success. `transact_system_call()` delegates transparently without appending to the tx-body trace buffer.
- [ ] 3.2 Define associated types on `TracingOpEvm`: `type DB = DB`, `type Tx = OpTransaction<TxEnv>`, `type Error = EVMError<DB::Error, OpTransactionError>`, `type HaltReason = OpHaltReason`, `type Spec = OpSpecId`, `type BlockEnv = BlockEnv`, `type Precompiles = PrecompilesMap`, `type Inspector = I`. Verify these match `OpEvm`'s associated types.
- [ ] 3.3 Define `TracingEvmFactory` struct with `traces: Arc<Mutex<Vec<EvmState>>>`. Implement `EvmFactory` with `type Evm<DB, I> = TracingOpEvm<DB, I>`. Both `create_evm` and `create_evm_with_inspector` delegate to `OpEvmFactory` then wrap the result.
- [ ] 3.4 Add unit tests for `TracingEvmFactory`: construct factory, create EVM, execute a simple transaction against an `InMemoryDB`, verify trace buffer contains exactly one entry with the expected touched addresses/slots. Test multiple transactions produce ordered tx-body trace entries. Test block-level `transact_system_call()` delegation does not append an extra trace entry. Test error propagation (failed tx → no trace entry).
- [ ] 3.5 Run full `kailua-kona` test suite — verify zero regressions.

## 4. CachedExecutor EvmFactory generics

- [ ] 4.1 Modify `CachedExecutor::new()` in `crates/kona/src/executor.rs`: change the specialized `impl` block from `CachedExecutor<KonaExecutor<'a, P, H, OpEvmFactory>>` to accept a generic `Evm: EvmFactory + Send + Sync + Clone + Debug` parameter, becoming `CachedExecutor<KonaExecutor<'a, P, H, Evm>>`. Pass the factory through to `KonaExecutor::new()`.
- [ ] 4.2 Update all call sites of `CachedExecutor::new()` in `crates/kona/src/client/core.rs` to pass `OpEvmFactory::default()` explicitly (preserving current behavior).
- [ ] 4.3 Run existing `test_cached_executor` and all `kailua-kona` tests — verify identical behavior. The refactor must be purely mechanical with no behavior change.

## 5. Chunk witness construction (host-side)

- [ ] 5.1 Create `crates/kona/src/chunk/mod.rs` module. Define `ChunkWitness` struct: `block_number: u64`, `chunk_index: u16`, `total_chunks: u16`, `tx_start: u16`, `tx_count: u16`, `cache: Cache` (chunk-start flat state after the block prelude), `evm_state: EvmAccumulatorState` (pre-chunk tx-body accumulators), `agreed_l2_output_root: B256`, `config_hash: B256`, `fpvm_image_id: B256`, `payout_recipient: Address`. Add rkyv serialization.
- [ ] 5.2 Implement `group_transactions_into_chunks(tx_count: usize, max_txs_per_chunk: usize) -> Vec<Range<usize>>`. Returns non-overlapping ranges covering all transactions.
- [ ] 5.3 Add unit tests for `group_transactions_into_chunks`: even division, remainder, single chunk, single transaction per chunk, max >= total.
- [ ] 5.4 Implement `build_chunk_witnesses(traces: &[EvmState], post_prelude_cache: &Cache, block_txs: &[...], max_txs_per_chunk: usize, ...) -> Vec<ChunkWitness>`. For each chunk: advance a chunk-boundary state from the post-prelude cache, carry forward full `DbAccount` metadata and storage writes from prior chunks, identify the addresses / storage slots / contracts / block hashes needed by the chunk, populate the chunk-start Cache snapshot, and compute EVM accumulator state for chunk boundaries.
- [ ] 5.5 Add unit tests for `build_chunk_witnesses`: single-chunk block (witness == post-prelude state), two-chunk block where chunk_1 reads chunk_0's storage write, same sender in chunk_0 and chunk_1 requiring nonce/balance carry-forward, contract creation in chunk_0 observed in chunk_1, selfdestruct in chunk_0 observed in chunk_1, contract bytecode inclusion, block_hash inclusion, EVM accumulator continuity between chunks.
- [ ] 5.6 Run full `kailua-kona` test suite — verify zero regressions.

## 6. Chunk proving guest mode

- [ ] 6.1 Create `crates/kona/src/chunk/prove.rs`. Implement `run_chunk_proof(witness: ChunkWitness) -> ProofJournal` that: (a) builds `CacheDB<PanicDB>` from witness.cache, (b) computes pre_db_hash, (c) sets up the EVM environment from witness block context, (d) executes only the chunk's ordered transaction body against the chunk-start cache (with no block-level prelude or epilogue), (e) computes post_db_hash and post_evm_state_hash, (f) computes chunk_trace, (g) returns ProofJournal with chunk sentinel.
- [ ] 6.2 Define `PanicDB` struct implementing `Database` that panics on `basic()`, `storage()`, `code_by_hash()`, and `block_hash()` with descriptive error messages including the requested address/slot.
- [ ] 6.3 Add unit tests for `PanicDB`: each method panics with expected message.
- [ ] 6.4 Add integration test for `run_chunk_proof`: construct a ChunkWitness for a known set of simple transactions (transfers, storage writes), execute, verify the returned ProofJournal has correct sentinel values and the precondition_hash matches manual computation of chunk_trace.
- [ ] 6.5 Add integration test: multi-chunk block. Build witnesses for 2 chunks of the same block. Execute both. Verify post_db_hash of chunk_0 matches pre_db_hash in chunk_1's witness. Verify post_evm_hash of chunk_0 matches pre_evm_hash of chunk_1.
- [ ] 6.6 Add test: missing state in witness triggers PanicDB. Construct a ChunkWitness that is intentionally incomplete. Verify the chunk proof panics.
- [ ] 6.7 Run full `kailua-kona` test suite — verify zero regressions.

## 7. Chunk aggregation in CachedExecutor

- [ ] 7.1 Define `BlockChunkData` struct in `crates/kona/src/chunk/aggregate.rs`: `chunks: Vec<ChunkMetadata>` where `ChunkMetadata { tx_count: u16, post_db_hash: B256, post_evm_state: EvmAccumulatorState }`. Define `BlockChunkWitness { post_tx_cache: Cache, final_evm_state: EvmAccumulatorState, chunk_data: Vec<ChunkMetadata> }`.
- [ ] 7.2 Implement `verify_and_aggregate_chunks(trie_db: &mut TrieDB, witness: BlockChunkWitness, transactions: &[...], proven_journals: &HashSet<Digest>, fpvm_image_id: B256, ...) -> Result<BlockBuildingOutcome>`. Follows the block aggregation flow: load initial trie state → apply block prelude once → hash → verify each chunk → load post-transaction cache → verify hash → apply block epilogue once → diff → trie root → build header.
- [ ] 7.3 Add unit tests for `verify_and_aggregate_chunks`: single chunk (trivial aggregation), two chunks with correct hash chain, hash mismatch between chunks causes failure, post-transaction cache hash mismatch causes failure, final evm state hash mismatch causes failure, prelude is applied exactly once, epilogue is applied exactly once.
- [ ] 7.4 Add `CachedExecutor` fields: `tx_chunk_witnesses: Vec<BlockChunkWitness>` and `proven_journals: Option<Arc<HashSet<Digest>>>` (guest-only, behind `#[cfg(target_os = "zkvm")]` or generic). Extend `execute_payload()` to check for chunk data before the existing cache-hit and execution paths.
- [ ] 7.5 Add integration test: construct a CachedExecutor with chunk data for a known block. Call `execute_payload()`. Verify the returned `BlockBuildingOutcome` matches monolithic execution output (same state_root, receipts_root, gas_used, logs_bloom).
- [ ] 7.6 Run full `kailua-kona` test suite — verify zero regressions.

## 8. Witness struct and guest entry point changes

- [ ] 8.1 Add `tx_chunk_witnesses: Vec<BlockChunkWitness>` field to `Witness` struct in `crates/kona/src/witness.rs` with rkyv serialization. Default to empty vec (backward compatible).
- [ ] 8.2 Verify existing witness serialization/deserialization tests pass with the new field defaulting to empty. Add test for round-trip with non-empty chunk data.
- [ ] 8.3 Modify `run_core_client()` in `crates/kona/src/client/core.rs` to thread `tx_chunk_witnesses` and `proven_journals` into `CachedExecutor` construction when chunk data is present in the witness.
- [ ] 8.4 Add chunk execution mode routing in guest entry points (`build/risczero/*/src/main.rs`): when the witness has `chunk_execution_mode` set, call `run_chunk_proof()` instead of `run_stateless_client()`.
- [ ] 8.5 Run full `kailua-kona` test suite and existing guest integration tests — verify zero regressions.

## 9. Prover-side chunk dispatch

- [ ] 9.1 Add `max_txs_per_chunk: usize` field to `ProvingArgs` in `crates/prover/src/args.rs` with default `usize::MAX` and `#[clap(long)]`.
- [ ] 9.2 In `crates/prover/src/tasks.rs`, add chunk dispatch logic: when `max_txs_per_chunk < block_tx_count`, pre-execute with `TracingEvmFactory`, apply the block prelude once to obtain the post-prelude flat cache, call `build_chunk_witnesses()`, dispatch chunk proof jobs in parallel via `seek_proof()`, collect receipts.
- [ ] 9.3 After chunk receipts are collected, assemble the `BlockChunkWitness` for the aggregation proof: initial trie preimages + chunk metadata + post-transaction cache + final evm state + chunk receipts appended to `stitched_proofs`.
- [ ] 9.4 Add integration test: mock a block with known transactions, set `max_txs_per_chunk`, verify chunk witnesses are constructed correctly and chunk proofs are dispatched.
- [ ] 9.5 Add integration test: verify that `max_txs_per_chunk = usize::MAX` produces identical behavior to the current code (no chunking path activated).

## 10. End-to-end validation

- [ ] 10.1 Create an end-to-end test that proves a known block both monolithically and via 2-chunk decomposition. Verify both produce identical `ProofJournal` (same precondition_hash at the block level, same agreed/claimed output roots, same block number).
- [ ] 10.2 Create an end-to-end test with 3+ chunks where cross-chunk state dependencies exist (chunk_1 reads chunk_0's write, chunk_2 depends on prior chunk account metadata such as nonce, balance, or contract creation). Verify the aggregation proof produces correct state_root.
- [ ] 10.3 Create an end-to-end test combining block-level stitching (`max_block_executions`) with transaction chunking (`max_txs_per_chunk`). Verify the composed proof is valid.
- [ ] 10.4 Run the full Kailua test suite (all crates) and verify zero regressions across the entire codebase.

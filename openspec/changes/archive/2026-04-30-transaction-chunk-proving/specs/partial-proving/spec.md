## ADDED Requirements

### Requirement: Partial execution-only mode in `run_core_client` triggered by `l1_head` sentinel

`run_core_client()` in `kailua-kona` at `crates/kona/src/client/core.rs` SHALL recognize a third branch — **PARTIAL EXECUTION** — activated when `boot.l1_head == B256::repeat_byte(0xFF)`. This branch is selected before the `EXECUTION ONLY` (`l1_head == 0x00..00`) and `DERIVATION & EXECUTION` branches. The partial witness flows through `Witness.pe_witness: Option<PartialExecutionWitness>` → `run_stateless_client()` → `run_stitching_client()` → `run_core_client()` as a dedicated parameter.

#### Scenario: `l1_head == 0xFF..FF` selects partial mode
- **WHEN** `run_core_client()` is called and `boot.l1_head == B256::repeat_byte(0xFF)`
- **THEN** it enters the PARTIAL EXECUTION branch before any safe-head / derivation / execution-only logic runs

#### Scenario: partial witness required in partial mode
- **WHEN** partial mode is activated and `pe_witness` is `None`
- **THEN** `run_core_client()` returns an error (`"partial witness required in partial mode"`)

#### Scenario: partial mode returns the partial precondition
- **WHEN** partial execution completes successfully
- **THEN** `run_core_client()` returns `(boot, Precondition::default().partial(pe_trace))` where `pe_trace = compute_pe_trace(results_hash, block_ctx_hash, expected_state_hash)`

### Requirement: Partial guest executes the carried transactions against the real `TrieDB` seeded with a `CacheState` prestate

The partial guest SHALL execute only the ordered transactions carried in the witness against a `State` backed by `TrieDB` (the same state primitive the aggregation path uses). The `CacheState` carried in `PartialExecutionWitness.cache` SHALL be installed as the state's pre-state layer via `State::builder().with_cached_prestate(cache).build()`. Any read that misses both the cached prestate and the trie SHALL propagate through the standard `TrieDB` / oracle path — not a dedicated "PanicDB".

Before execution, the guest SHALL call `validate_cache(&cache)` to verify every top-level contract entry AND every inline account bytecode against its `code_hash` key (`bytecode.hash_slow() == code_hash`, with the workaround that a zero code_hash requires empty bytecode). A mismatch SHALL cause witness validation to fail before any transaction is executed.

The guest SHALL assert `boot.agreed_l2_output_root == boot.claimed_l2_output_root` (partials do not advance L2 state) and that `boot.claimed_l2_block_number` matches the safe-head header's block number.

#### Scenario: contract validation runs before execution
- **WHEN** `validate_cache(&cache)` is called and any contract bytecode does not hash to its `code_hash` key
- **THEN** the guest aborts before executing any transaction

#### Scenario: agreed and claimed output roots must be equal
- **WHEN** `boot.agreed_l2_output_root != boot.claimed_l2_output_root` in partial mode
- **THEN** the guest asserts and aborts (partials do not advance L2 state)

#### Scenario: claimed block number matches safe-head header
- **WHEN** `boot.claimed_l2_block_number != safe_head.number` in partial mode
- **THEN** the guest asserts and aborts

### Requirement: Partial guest re-derives `expected_state` against the seeded state before executing transactions

After building the `State` and validating the cache, BEFORE running any transaction the guest SHALL call `capture_required_expected_state(&mut state)` to materialize the partial's starting OP-specific L1Block snapshot. The guest SHALL feed the result into `hash_expected_state(...)` to compute `expected_state_hash`.

The witness does NOT carry `expected_state` explicitly — `cache_results` (during witness construction) seeds the host's snapshot into `cache` alongside the chunk's per-tx prestate. The guest's re-derive against the seeded `State` reproduces the host's snapshot byte-for-byte by construction (verified by the `witness_cache_round_trips_expected_state` test), so the resulting `expected_state_hash` matches the host's pre-hash input.

#### Scenario: re-derive against the seeded cache reproduces the host snapshot
- **WHEN** the guest builds `State::with_cached_prestate(witness.cache)` and runs `capture_required_expected_state`
- **THEN** the resulting snapshot equals the snapshot the host captured (and folded into `cache_results`) when building the witness

#### Scenario: `expected_state_hash` binds the partial proof to its starting L1Block view
- **WHEN** a witness ships a `cache` whose L1Block contract slots differ from the live aggregation DB's view at the partial's parent block
- **THEN** the guest's re-derive produces a different snapshot, the resulting `pe_trace` differs from the aggregation-side reconstruction, and `env::verify()` rejects the assumption

### Requirement: Partial guest drives execution through `OpBlockExecutor` with `CachedEvmFactory` for trace capture

The partial guest SHALL construct `OpBlockExecutor::new(cached_evm_factory.create_evm(&mut state, evm_env), op_block_ctx, rollup_config, OpAlloyReceiptBuilder::default())`, where `cached_evm_factory = CachedEvmFactory::new_with_traces(Vec::new(), Some(traces))`. It SHALL call `op_block_executor.execute_transaction(wrapped)` for each `WithEncoded<Recovered<OpTxEnvelope>>` reconstructed from `witness.transactions`. It SHALL NOT skip `apply_pre_execution_changes` or `finish`, and SHALL NOT seed any accumulator fields (`gas_used`, `da_footprint_used`, `receipts`) — the executor is used in its standard form. (Partials do not need to carry EVM-accumulator continuity because the aggregation path reruns the block through the same executor with `CachedEvm` serving the cached results.)

After the tx loop, the guest SHALL drain the factory's shared trace buffer via `take_all_block_traces()`, extract the first per-block inner `Vec<PartialExecutionTrace>`, unzip into `(captured_tx_hashes, captured_results)`, and compute `results_hash = hash_results(&captured_tx_hashes, &captured_results)`.

#### Scenario: `transact_system_call` calls do not append tx-body traces
- **WHEN** the executor runs block-level prelude / epilogue work during a partial proof
- **THEN** those calls go through `CachedEvm::transact_system_call` → inner `OpEvm` and do not append entries to the trace collector

#### Scenario: every tx-body transaction is captured exactly once
- **WHEN** `N` transactions from `witness.transactions` are executed successfully
- **THEN** `captured_results.len() == N` and entries appear in execution order paired with their EIP-2718 `keccak256` tx hashes

#### Scenario: invalid tx encoding or signature aborts
- **WHEN** any entry of `witness.transactions` fails `OpTxEnvelope::decode_2718_exact` or `try_into_recovered`
- **THEN** the guest returns an error without emitting a partial journal

### Requirement: Partial guest commits to `results_hash + block_ctx_hash + expected_state_hash`

The partial guest SHALL compute its `pe_trace = compute_pe_trace(results_hash, block_ctx_hash, expected_state_hash)` where:

- `results_hash = hash_results(&captured_tx_hashes, &captured_results)`
- `block_ctx_hash = hash_block_ctx(&witness.block_env, &witness.op_block_ctx)`
- `expected_state_hash = hash_expected_state(&capture_required_expected_state(&mut state))`

The guest SHALL emit `Precondition::default().partial(pe_trace)` so the standard `Digestible` path yields the journal's `precondition_hash`.

#### Scenario: `pe_trace` is the three-input SHA256
- **WHEN** the partial guest emits its precondition
- **THEN** it equals `SHA256(results_hash || block_ctx_hash || expected_state_hash)`

#### Scenario: `results_hash` binding rejects witness-substituted results
- **WHEN** a witness ships `results` or `tx_hashes` that differ from those the partial guest captured on this execution
- **THEN** the partial journal's `precondition_hash` differs from what the aggregation side reconstructs, so `env::verify()` rejects the assumption

#### Scenario: `block_ctx_hash` binding rejects forged block context
- **WHEN** a witness ships `block_env` / `op_block_ctx` fields (timestamp, basefee, prevrandao, coinbase, blob pricing, parent_hash, extra_data, etc.) that differ from what the aggregation side sees for this block
- **THEN** the aggregation side reconstructs a different `pe_trace`, so `env::verify()` rejects the assumption

#### Scenario: `expected_state_hash` binding rejects forged L1Block view
- **WHEN** a witness ships a `cache` whose L1Block slot values differ from the live aggregation DB at the partial's parent block
- **THEN** the guest's re-derive produces a different snapshot, the partial proof and the aggregation-side reconstruction commit to different `pe_trace` values, and `env::verify()` rejects the assumption

### Requirement: Partial guest emits a standard `ProofJournal` with the partial sentinel

The partial guest SHALL emit a `ProofJournal` with:

| Field | Value |
|-------|-------|
| `payout_recipient` | from witness |
| `precondition_hash` | digest of `Precondition::default().partial(pe_trace)` |
| `l1_head` | `B256::repeat_byte(0xFF)` (partial sentinel) |
| `agreed_l2_output_root` | `boot.agreed_l2_output_root` |
| `claimed_l2_output_root` | same as `agreed_l2_output_root` |
| `claimed_l2_block_number` | parent block number |
| `config_hash` | from witness |
| `fpvm_image_id` | from witness |

The `KonaStitchingClient::run_stitching_client` short-circuits when it sees `boot.l1_head == 0xFF..FF` after `run_core_client` returns: it constructs the journal directly via `ProofJournal::new(fpvm_image_id, payout_recipient_address, B256::new(precondition.digest().into()), &boot)` and returns immediately, bypassing `stitch_executions` / `stitch_partial_executions` / `stitch_boot_info`.

#### Scenario: partial journal is distinguishable by `l1_head`
- **WHEN** a receipt's journal is decoded
- **THEN** `l1_head == 0xFF..FF` identifies it as a partial proof (vs `0x00..00` for execution-only or a real L1 hash for derivation)

#### Scenario: agreed equals claimed
- **WHEN** a partial journal is emitted
- **THEN** `agreed_l2_output_root == claimed_l2_output_root`

#### Scenario: block number is the parent's
- **WHEN** a partial journal is emitted for transactions belonging to block `P + 1`
- **THEN** `claimed_l2_block_number == P`

#### Scenario: stitching short-circuits in partial mode
- **WHEN** `run_stitching_client` observes `boot.l1_head == 0xFF..FF` after `run_core_client` returns
- **THEN** it returns the partial proof's journal immediately, without invoking any of `stitch_executions`, `stitch_partial_executions`, or `stitch_boot_info`

## ADDED Requirements

### Requirement: Chunk execution-only mode in run_core_client triggered by l1_head sentinel

**Crate:** `kailua-kona` (`crates/kona/src/client/core.rs`)

The `run_core_client()` function SHALL support a chunk execution-only mode, activated when `boot.l1_head == B256::from([0xFF; 32])`. This is a third branch alongside the existing execution-only mode (`l1_head == 0x00..00`) and the derivation + execution mode (any real L1 hash). The chunk witness flows through the existing pipeline: `Witness.chunk_witness` → `run_stateless_client()` → `run_stitching_client()` → `run_core_client()`.

#### Scenario: l1_head sentinel selects chunk mode
- **WHEN** `run_core_client()` is called and `boot.l1_head == 0xFF..FF`
- **THEN** it enters the chunk execution-only branch (not derivation, not execution-only)

#### Scenario: chunk witness required in chunk mode
- **WHEN** chunk mode is activated
- **THEN** a `ChunkWitnessData` must be available from the witness (panic otherwise)

#### Scenario: chunk mode returns chunk precondition
- **WHEN** chunk execution completes
- **THEN** `run_core_client()` returns `(boot, Precondition::default().chunk(chunk_trace))`

### Requirement: Chunk guest executes only the transaction body against CacheDB with PanicDB fallback

The chunk execution-only mode SHALL execute only a subset of a block's ordered transaction body against a `CacheDB<PanicDB>` (or equivalent in-memory DB backed by a panicking fallback). The witness MUST include the ordered chunk transactions and the full block execution context needed to instantiate the same EVM / executor environment as monolithic execution. The witness cache for `chunk_0` represents the post-prelude block state, and the final chunk stops at the post-last-transaction, pre-epilogue state. All required state MUST be pre-loaded in the cache. If any transaction reads state not present in the cache, the guest SHALL panic. Before computing `pre_db_hash` or executing any transaction, the guest SHALL validate every cached contract entry against its `code_hash` key (for example `bytecode.hash_slow() == code_hash`) and fail on mismatch.

#### Scenario: all state present, execution succeeds
- **WHEN** the chunk witness contains all accounts and storage slots that the chunk's transactions will access
- **THEN** all transactions execute successfully and the chunk proof completes

#### Scenario: malformed witness contract is rejected before execution
- **WHEN** the chunk witness contains a cached contract whose bytecode does not hash to its `code_hash` key
- **THEN** the guest fails before hashing or executing the chunk

#### Scenario: missing state causes panic
- **WHEN** a transaction in the chunk attempts to read a storage slot not present in the CacheDB cache
- **THEN** the PanicDB fallback is triggered and the guest panics (note: revm's `EmptyDB` returns defaults instead of panicking, so a custom `PanicDB` implementing `DatabaseRef` with `type Error = Infallible` is required)

#### Scenario: system transactions handled like regular transactions
- **WHEN** a chunk includes system deposit transactions (L1 info deposit, etc.)
- **THEN** they are executed in order like any other transaction, with their state provided in the cache

#### Scenario: chunk_0 starts from the post-prelude state
- **WHEN** the first chunk begins execution
- **THEN** the witness cache already includes any once-per-block prelude state changes, and the chunk guest does not replay those changes

#### Scenario: final chunk stops before epilogue
- **WHEN** the last chunk completes execution
- **THEN** its `post_db_hash` and `post_evm_state_hash` describe the post-last-transaction, pre-epilogue state, and the chunk guest does not apply the epilogue

#### Scenario: witness carries the execution inputs needed by the guest
- **WHEN** a chunk proof is executed
- **THEN** the witness provides both the ordered chunk transactions and the block execution context needed to execute them and compute `tx_hash`

#### Scenario: state-clear semantics still match monolithic execution
- **WHEN** the chunk guest skips block-level prelude replay
- **THEN** it still configures the state-clear flag from the same block / hardfork predicate used by monolithic execution, rather than hardcoding `without_state_clear()`

### Requirement: Chunk guest computes pre/post memory DB hashes

The chunk guest SHALL compute the SHA256 hash of the canonical execution-relevant memory DB state before executing any transactions (`pre_db_hash`) and after executing all transactions (`post_db_hash`), using the canonical encoding defined in memory-db-hashing.

#### Scenario: pre_db_hash reflects loaded chunk-start state
- **WHEN** the chunk guest loads the memory DB from the witness
- **THEN** `pre_db_hash = SHA256(canonical_encode(cache))` before any transaction executes

#### Scenario: post_db_hash reflects modified state
- **WHEN** all chunk transactions have executed and committed their state changes
- **THEN** `post_db_hash` reflects the effective post-transaction flat state, computed by hashing the canonical `Cache` view of that state; if execution ran on top of a preloaded `CacheDB` base cache, this canonical view includes untouched witness entries by overlaying the final live `CacheState` projection and block hashes onto the initial witness `Cache`

#### Scenario: chunk_0 pre_db_hash is post-prelude
- **WHEN** `chunk_0` is proven
- **THEN** its `pre_db_hash` commits to the post-prelude flat state

#### Scenario: final chunk post_db_hash is pre-epilogue
- **WHEN** the last chunk is proven
- **THEN** its `post_db_hash` commits to the post-last-transaction, pre-epilogue flat state

### Requirement: Chunk guest computes pre/post EVM state hashes

The chunk guest SHALL compute the SHA256 hash of the EVM state accumulators before execution (`pre_evm_state_hash`) and after execution (`post_evm_state_hash`).

#### Scenario: pre_evm_state_hash loaded from witness
- **WHEN** chunk_0 starts execution
- **THEN** `pre_evm_state_hash` is computed from the initial tx-body EVM accumulators after the prelude phase (typically zeroed)

#### Scenario: pre_evm_state_hash for non-first chunk
- **WHEN** chunk_i (i > 0) starts execution
- **THEN** `pre_evm_state_hash` is computed from the EVM accumulators loaded from the witness (representing the post-state of chunk i-1)

#### Scenario: post_evm_state_hash reflects accumulated values
- **WHEN** all chunk transactions have executed
- **THEN** `post_evm_state_hash` is computed from the updated accumulators (cumulative_gas_used, da_footprint_used, blob_gas_used, logs_bloom, receipts)

#### Scenario: EVM accumulators are seeded before transaction execution
- **WHEN** the chunk guest constructs its block executor
- **THEN** its cumulative gas, DA footprint, and receipts accumulators start from the values committed by `pre_evm_state_hash`, so that per-transaction prechecks (block-available-gas, Jovian DA footprint budget) see the same budget a monolithic executor would see at the same point in block execution

### Requirement: Chunk guest computes tx_hash

The chunk guest SHALL compute `tx_hash = SHA256(canonical_encode(transactions))` where `transactions` is the ordered list of transactions in the chunk.

#### Scenario: tx_hash is deterministic
- **WHEN** the same transactions in the same order are provided
- **THEN** `tx_hash` is identical across invocations

### Requirement: Chunk guest commits to per-tx `results_hash` via `chunk_trace`

The chunk guest SHALL capture the full `ResultAndState` of each executed transaction (via a tracing EVM wrapper around the inner `OpEvm`, e.g. `TracingOpEvmFactory`) and SHALL fold the canonical SHA256 hash of that ordered trace — `results_hash = hash_results(traces)` — into `chunk_trace`. The canonical encoding excludes transient revm fields (`transaction_id`, `is_cold`, `Account.original_info`, `AccountInfo.code`) and sorts per-address and per-storage-slot entries deterministically, matching [`crate::precondition::chunking::hash_results`].

This binding is what permits the aggregation guest (`ChunkingEvm`) to replay `Chunk.results` verbatim rather than re-executing transactions: any alteration of the results vec changes `results_hash`, which changes `chunk_trace`, which changes the chunk `ProofJournal` identity, causing `env::verify()` on the aggregation side to reject the swapped assumption.

#### Scenario: chunk_trace includes results_hash and block_ctx_hash
- **WHEN** the chunk guest emits its `chunk_trace`
- **THEN** it is computed as `SHA256(tx_hash || pre_db_hash || post_db_hash || pre_evm_hash || post_evm_hash || results_hash || block_ctx_hash)` where `results_hash = hash_results(captured_traces)` and `block_ctx_hash = hash_block_ctx(&block_env, &op_block_ctx)`

#### Scenario: block_ctx_hash is independent of transient EVM accumulator state
- **WHEN** two chunks executing the same transaction set under the same `BlockEnv` / `OpBlockExecutionCtx` but with different pre-chunk accumulator seeds compute their `block_ctx_hash`
- **THEN** both produce the same `block_ctx_hash` — it depends only on the block execution context, not on per-chunk accumulator state (which is already bound via `pre_evm_hash` / `post_evm_hash`)

#### Scenario: chunk_trace changes if any block_env or op_block_ctx field is altered
- **WHEN** any field in `block_env` (number, beneficiary, timestamp, gas_limit, basefee, difficulty, prevrandao, blob_excess_gas_and_price) or `op_block_ctx` (parent_hash, parent_beacon_block_root, extra_data) changes
- **THEN** `block_ctx_hash` changes → `chunk_trace` changes → the chunk's `ProofJournal` identity changes, causing `env::verify()` on the aggregation side to reject the mismatched journal

#### Scenario: trace capture is per ordered-tx-body transaction
- **WHEN** the chunk guest executes `N` transactions through `OpBlockExecutor::execute_transaction`
- **THEN** exactly `N` `ResultAndState` entries are captured, in execution order; block-level system calls (`transact_system_call`) are NOT captured

#### Scenario: results_hash is independent of transient fields
- **WHEN** two logically-equivalent `ResultAndState` entries differ only in `transaction_id`, `is_cold`, or `Account.original_info`
- **THEN** they produce the same `results_hash`, so the same logical trace captured under different executor contexts still matches

#### Scenario: results_hash changes on any material alteration
- **WHEN** any `result`'s variant, gas values, logs, output bytes, state accounts, status bits, storage slots, or ordering changes
- **THEN** `results_hash` differs, `chunk_trace` differs, and the reconstructed journal no longer matches the chunk proof

#### Scenario: aggregation reconstructs the same `results_hash`
- **WHEN** `stitch_chunks` runs on `Chunk.results` supplied by the witness
- **THEN** it recomputes `hash_results(&chunk.results)` and uses the six-input `compute_chunk_trace` to rebuild the expected journal; `env::verify` accepts the chunk proof only if both sides agree byte-for-byte on `results_hash`

### Requirement: Chunk guest emits standard ProofJournal with chunk sentinel

The chunk guest SHALL emit a `ProofJournal` with:
- `payout_recipient`: from witness
- `precondition_hash`: digest of `Precondition::default().chunk(chunk_trace)` where `chunk_trace = SHA256(tx_hash || pre_db_hash || post_db_hash || pre_evm_state_hash || post_evm_state_hash || results_hash || block_ctx_hash)`. The `results_hash` binding commits to the canonical SHA256 encoding of the per-transaction `ResultAndState` trace captured during chunk execution; it is what lets the aggregation proof replay `Chunk.results` without re-execution while still authenticating the exact per-tx transition. The `block_ctx_hash` binding commits to the canonical SHA256 encoding of the exact `BlockEnv` and `OpBlockExecutionCtx` under which the chunk guest executed, so env-sensitive opcode results (BASEFEE / PREVRANDAO / NUMBER / COINBASE / TIMESTAMP / BLOBBASEFEE / BLOCKHASH / EIP-4788 beacon root / EIP-2935 ring / Holocene-Jovian EIP-1559 params in extra_data) cannot be forged under a different context.
- `l1_head`: `B256::from([0xFF; 32])` (chunk sentinel)
- `agreed_l2_output_root`: current agreed L2 output root from witness
- `claimed_l2_output_root`: same as `agreed_l2_output_root`
- `claimed_l2_block_number`: parent block number
- `config_hash`: from witness
- `fpvm_image_id`: from witness

#### Scenario: chunk ProofJournal is distinguishable by l1_head
- **WHEN** a receipt's journal is decoded as ProofJournal
- **THEN** `l1_head == 0xFF..FF` identifies it as a chunk proof (vs `0x00..00` for execution-only, or a real L1 hash for derivation)

#### Scenario: agreed equals claimed for chunks
- **WHEN** a chunk ProofJournal is constructed
- **THEN** `agreed_l2_output_root == claimed_l2_output_root` (chunk does not advance L2 state)

#### Scenario: block number is parent
- **WHEN** a chunk ProofJournal is constructed for a block being built on top of parent block P
- **THEN** `claimed_l2_block_number == P.number`

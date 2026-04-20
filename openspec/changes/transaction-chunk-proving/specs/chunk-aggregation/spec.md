## ADDED Requirements

### Requirement: Chunk struct represents a proven transaction chunk (analogous to Execution)

**Crate:** `kailua-kona` (`crates/kona/src/executor.rs`, co-located with `Execution`)

The system SHALL define a `Chunk` struct that represents a proven transaction chunk, analogous to `Execution` for blocks. Fields:

- `agreed_db: B256` — pre-chunk DB state hash
- `agreed_evm: B256` — pre-chunk EVM accumulator hash
- `tx_count: u16` — number of transactions in this chunk
- `tx_hash: B256` — hash of the chunk's transactions (for matching and proof verification)
- `results: Vec<ResultAndState>` — full per-transaction execution results (gas, logs, output, state changes)
- `evm_state: EvmAccumulatorState` — post-chunk EVM accumulator state
- `claimed_db: B256` — post-chunk DB state hash
- `claimed_evm: B256` — post-chunk EVM accumulator hash

Each `ResultAndState` entry contains the complete per-transaction `ExecutionResult` and `EvmState` (state diff), identical to what the EVM would have returned during monolithic execution. The block executor processes these through its normal commit path.

#### Scenario: Chunk mirrors Execution pattern
- **WHEN** a `Chunk` is instantiated
- **THEN** it carries pre-state commitments (`agreed_db`, `agreed_evm`), outputs (`results`, `evm_state`), and post-state commitments (`claimed_db`, `claimed_evm`) — analogous to `Execution`'s `agreed_output`, `artifacts`, `claimed_output`

#### Scenario: results length matches tx_count
- **WHEN** a `Chunk` is constructed
- **THEN** `results.len() == tx_count as usize`

### Requirement: ChunkingEvm wraps inner Evm and returns pre-computed results (analogous to TracingOpEvm)

**Crate:** `kailua-kona`

The system SHALL provide a `ChunkingEvm<E: Evm>` struct that wraps an inner `E: Evm` and implements the `Evm` trait. It is instantiated with a `Vec<Chunk>` (reversed, popped from end) and tracks the current transaction index. The `ChunkingEvmFactory` produces `ChunkingEvm<OpEvm<...>>` instances, following the same factory pattern as `TracingEvmFactory`.

- `transact_raw()`: If the next chunk is active (or a new chunk starts at this tx index), return the next pre-computed `ResultAndState` from the chunk's `results`. The block executor's normal commit path applies `result.state` to `State<TrieDB>`. If no chunk is active and no pending chunk matches, delegate to the inner EVM for actual execution.
- `transact_system_call()`: Always delegates to the inner EVM without modification (prelude and epilogue run normally).
- All other `Evm` trait methods: Delegate to the inner EVM.

This serves both roles with the same code:
- **Chunk proving** (empty chunk vec): all `transact_raw()` calls delegate to the inner EVM (normal execution).
- **Chunk aggregation** (populated chunk vec): all `transact_raw()` calls return pre-computed results.

#### Scenario: transact_raw returns pre-computed result during aggregation
- **WHEN** a chunk is active and `transact_raw()` is called
- **THEN** the next `ResultAndState` from `chunk.results` is returned without invoking the inner EVM
- **AND** the block executor commits `result.state` to `State<TrieDB>` through its normal path

#### Scenario: transact_system_call delegates to inner EVM
- **WHEN** `transact_system_call()` is called during prelude or epilogue
- **THEN** the call delegates to the inner `OpEvm`, executing normally against `State<TrieDB>`

#### Scenario: empty chunk vec delegates all transact_raw to inner
- **WHEN** ChunkingEvm has no chunks
- **THEN** all `transact_raw()` calls delegate to the inner EVM (chunk proving mode)

#### Scenario: state remains consistent between system calls and chunk results
- **WHEN** the block executor commits `ResultAndState.state` from a pre-computed result
- **THEN** subsequent `transact_system_call()` calls (epilogue) see the updated state in `State<TrieDB>` because the EVM shares the same state reference

### Requirement: Chunk receipts are verified upfront in the stitching layer (like execution proofs)

**Crate:** `kailua-kona` (`crates/kona/src/client/stitching.rs`)

Chunk proof receipts SHALL be verified upfront by a `stitch_chunks()` function in `run_stitching_client()`, following the same pattern as `stitch_executions()`. This runs AFTER `run_core_client()` returns (which used the chunk data trusting it), and alongside the existing execution and boot info stitching. The `ChunkingEvm` does NOT verify receipts during `transact_raw()` — it trusts the chunk data because the stitching layer has already verified the receipts.

For each block's chunks, `stitch_chunks()` SHALL:
1. Verify hash chain continuity: `chunk[i].agreed_db == chunk[i-1].claimed_db` and `chunk[i].agreed_evm == chunk[i-1].claimed_evm`.
2. For each chunk, compute `chunk_trace = compute_chunk_trace(chunk.tx_hash, chunk.agreed_db, chunk.claimed_db, chunk.agreed_evm, chunk.claimed_evm)`.
3. Construct the expected `ProofJournal` using `chunk_trace` as precondition, `l1_head = 0xFF..FF`, and the block's `config_hash`, `fpvm_image_id`, `payout_recipient`, `agreed_l2_output_root`, `claimed_l2_block_number` from `BootInfo`.
4. Call `verify_stitching_journal()` for each chunk.

#### Scenario: stitch_chunks follows stitch_executions pattern
- **WHEN** `run_stitching_client()` completes
- **THEN** it calls `stitch_chunks()` alongside `stitch_executions()` and `stitch_boot_info()`, using the same `proven_fpvm_journals` set

#### Scenario: ChunkingEvm trusts chunk data
- **WHEN** `ChunkingEvm::transact_raw()` returns a pre-computed result
- **THEN** it does not independently verify the chunk proof (receipt verification was done upfront)

#### Scenario: hash chain break causes verification failure
- **WHEN** `chunk[i].agreed_db != chunk[i-1].claimed_db`
- **THEN** `stitch_chunks()` panics (invalid chunk chain)

#### Scenario: ProofJournal fields come from BootInfo
- **WHEN** `stitch_chunks()` reconstructs the expected ProofJournal for a chunk
- **THEN** it uses `config_hash`, `fpvm_image_id`, `payout_recipient`, `agreed_l2_output_root`, `claimed_l2_block_number` from the `BootInfo` returned by `run_core_client()`

### Requirement: Aggregation-side block-coherence verification via `verify_block_chunks`

**Crate:** `kailua-kona` (`crates/kona/src/precondition/chunking.rs`, `crates/kona/src/client/core.rs`)

Before the aggregation `ProofJournal` is finalized, `run_core_client()` SHALL invoke `verify_block_chunks` on every block whose witness supplied chunks. This helper enforces seven coherence checks against the derivation pipeline's actual output, closing the forgery vectors identified by the adversarial review:

1. **Output-root anchor** — `chunk.agreed_l2_output_root == derived_parent_output_root` and `chunk.parent_block_number == safe_head_number + block_position`. Uses the L2 output root as cryptographic anchor (collision-resistant commitment to state_root → all state), functionally equivalent to anchoring via `hash_cache_state(State<TrieDB>)` but without the canonicalization mismatch between chunk-witness `Cache` and aggregation `State<TrieDB>.cache`.
2. **tx_hash binding** — `compute_tx_hash(block_txs[cursor..cursor+tx_count]) == chunk.tx_hash` for each chunk, where `block_txs` comes from the derivation pipeline's `OpPayloadAttributes.transactions`.
3. **Full coverage** — `sum(chunk.tx_count) == block_txs.len()`; partial coverage is rejected to keep the aggregation model simple.
4. **evm_state self-consistency** — `hash_evm_state(&chunk.evm_state) == chunk.claimed_evm`.
5. **Receipts-root reconstruction** — `calculate_receipt_root(&last_chunk.evm_state.receipts) == block_header.receipts_root`.
6. **Block-context binding** — each chunk's carried `block_env` and `op_block_ctx` MUST match the derivation pipeline's sealed header (number, beneficiary, timestamp, gas_limit, basefee, difficulty, prevrandao/mix_hash, parent_hash, parent_beacon_block_root, extra_data). Combined with the `block_ctx_hash` folded into `chunk_trace` by `compute_chunk_trace`, this forces chunk proofs to have been generated under the exact execution context the aggregation is using — neither a forged context in the chunk witness nor a chunk proof generated under a wrong context can satisfy both checks.
7. **Blob-pricing authentication** — `chunk.block_env.blob_excess_gas_and_price` MUST equal the `BlobExcessGasAndPrice` that kona's block builder would derive from the derivation pipeline's parent header + the current block's hardfork (`spec_id`) — mirroring `kona/proof/executor/src/builder/env.rs::prepare_block_env`. `run_core_client` computes this expected value by calling `expected_blob_excess_gas_and_price(parent_header, spec_id)` and passes it into `verify_block_chunks`; the helper enforces exact equality. Without this check, the aggregation's replay-only semantic (we replay `chunk.results` rather than re-executing) would allow a prover to authenticate a chunk proof under a forged blob-pricing context (wrong BLOBBASEFEE, wrong blob-gas accounting) and have aggregation accept the wrong post-state. Internal pre/post EVM hash continuity is NOT a substitute because it is only self-consistency between attacker-controlled chunks.

#### Scenario: chunk from a different block is rejected
- **WHEN** a witness supplies a chunk whose `agreed_l2_output_root` or `parent_block_number` does not match the aggregation's derived parent for the block position
- **THEN** `verify_block_chunks` returns an error identifying the mismatched anchor, and `run_core_client` halts before emitting a `ProofJournal`

#### Scenario: chunk proven under forged block context is rejected
- **WHEN** a witness supplies a chunk whose `block_env` or `op_block_ctx` disagrees with the derivation pipeline's sealed header (e.g. different `timestamp`, `basefee`, `beneficiary`, `prevrandao`, `parent_hash`, `extra_data`)
- **THEN** the block-context cross-check in `verify_block_chunks` rejects the chunk — closing the env-forgery vector where an adversary runs chunks under an alternate context that produces different env-sensitive opcode results

#### Scenario: chunks failing to cover all block txs are rejected
- **WHEN** `sum(chunks[block_i].tx_count) < block_txs.len()`
- **THEN** `verify_block_chunks` returns a "full coverage required" error

#### Scenario: tx_hash tampering caught by derivation-side recomputation
- **WHEN** a witness's `chunk.tx_hash` does not equal `compute_tx_hash(block_txs[chunk_start..chunk_end])` computed over the derivation-produced tx bytes
- **THEN** `verify_block_chunks` rejects the chunk — closing the wrong-tx-sequence substitution vector

#### Scenario: authenticated receipts must reconstruct block receipts_root
- **WHEN** `calculate_receipt_root(&chunks[block_i].last().evm_state.receipts) != block_header.receipts_root`
- **THEN** `verify_block_chunks` rejects the chunks — the authenticated chunk receipts diverged from the block executor's produced receipts

#### Scenario: chunk with forged blob pricing is rejected
- **WHEN** a witness supplies a chunk whose `block_env.blob_excess_gas_and_price` does not equal `expected_blob_excess_gas_and_price(parent_header, spec_id)` — either absent when the hardfork expects `Some(...)`, present when it expects `None`, or present with the wrong `excess_blob_gas` / `blob_gasprice` (wrong update fraction for the hardfork)
- **THEN** `verify_block_chunks` rejects the chunk — closing the blob-pricing forgery vector where an adversary authenticates a chunk proof under a wrong BLOBBASEFEE / blob-gas-accounting context and has aggregation accept the wrong post-state

### Requirement: Trie root provides ultimate correctness guarantee

During chunk aggregation, correctness is assured by two complementary verification layers:

1. **Hash chain + receipt verification** (in `stitch_chunks()`): Verifies internal consistency between chunks — each chunk's proof was valid and the chain is continuous.
2. **Trie root computation** (in block executor): The per-tx `ResultAndState` instances are committed to `State<TrieDB>` through the normal block executor path. `state_root = trie_db.state_root(&bundle)` is computed inside the proof. If any chunk's state changes were incorrect, the trie root would differ from monolithic execution, producing an invalid block proof.

The aggregation proof does NOT need to compute flat cache hashes from the trie or compare them against chunk hashes. The flat cache hash chain verifies inter-chunk consistency; the trie root verifies global correctness.

#### Scenario: wrong chunk data produces wrong trie root
- **WHEN** a chunk's `ResultAndState` instances do not match what monolithic execution would produce
- **THEN** the trie root computed by `State<TrieDB>` differs from the correct value, and the block proof is invalid

#### Scenario: chunk proofs + trie root jointly guarantee correctness
- **WHEN** all chunk receipts are verified and the trie root matches monolithic execution
- **THEN** the block proof is valid and the state transition is correct

### Requirement: Block aggregation integrates with CachedExecutor via ChunkingEvmFactory

The `CachedExecutor` SHALL support chunk aggregation by accepting a `ChunkingEvmFactory` (constructed with chunk data) instead of `OpEvmFactory`. When chunks are present for blocks in the execution range, the caller provides `ChunkingEvmFactory` to `CachedExecutor::new()`. The factory produces `ChunkingEvm` instances that return pre-computed results during block execution.

The `ChunkingEvmFactory` SHALL hold per-block chunk data keyed by block number (or consumed sequentially matching block execution order). When `create_evm` or `create_evm_with_inspector` is called, it extracts the block number from `EvmEnv.block.number` to select the correct chunk vec for that block. If no chunks exist for a block, the resulting `ChunkingEvm` has an empty chunk vec and delegates all `transact_raw()` calls to the inner EVM (monolithic execution).

The existing `CachedExecutor` logic (cache-hit precedence, collection target, fallback) remains unchanged. The chunk aggregation is transparent at the executor level — it happens inside the EVM layer.

#### Scenario: CachedExecutor with ChunkingEvmFactory
- **WHEN** `CachedExecutor::new()` is called with `ChunkingEvmFactory` (which wraps `OpEvmFactory` + chunk data)
- **THEN** block execution uses `ChunkingEvm` instances that return pre-computed results for chunked transactions

#### Scenario: cache hit still takes precedence
- **WHEN** a block is in the execution cache AND has chunk data
- **THEN** the cache hit path returns the cached `BlockBuildingOutcome` (chunks are not consumed)

#### Scenario: transparent to block stitching
- **WHEN** a chunk-aggregated block is stitched with other blocks
- **THEN** the stitching succeeds identically to monolithic blocks — chunking is invisible at the stitching layer

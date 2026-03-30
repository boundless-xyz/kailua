## ADDED Requirements

### Requirement: max_txs_per_chunk configuration parameter

The prover SHALL accept a `max_txs_per_chunk: usize` argument with a default value of `usize::MAX` (chunking disabled). When set to a value less than the number of transactions in a block, the prover SHALL activate transaction chunk proving for that block.

#### Scenario: default disables chunking
- **WHEN** `max_txs_per_chunk` is not specified
- **THEN** all blocks are proven monolithically (existing behavior)

#### Scenario: explicit value enables chunking
- **WHEN** `max_txs_per_chunk = 4` and a block has 10 transactions
- **THEN** the prover creates 3 chunks ([0..3], [4..7], [8..9]) and proves them

#### Scenario: value >= block tx count produces single chunk
- **WHEN** `max_txs_per_chunk = 100` and a block has 5 transactions
- **THEN** the prover creates 1 chunk (effectively monolithic) but still goes through the chunk proving path

### Requirement: Prover dispatches chunk proofs in parallel

When chunking is active for a block, the prover SHALL:
1. Pre-execute the block with `TracingEvmFactory` to capture per-transaction traces.
2. Construct chunk witnesses using the traces.
3. Dispatch all chunk proof jobs concurrently (limited by `num_concurrent_proofs`).
4. Collect all chunk proof receipts.

#### Scenario: parallel dispatch
- **WHEN** a block is split into 4 chunks
- **THEN** up to 4 chunk proof jobs are dispatched concurrently (bounded by `num_concurrent_proofs`)

#### Scenario: chunk proof failure
- **WHEN** a chunk proof job fails
- **THEN** the prover reports the error (no partial results are used)

### Requirement: Prover assembles aggregation witness from chunk receipts

After all chunk proofs complete, the prover SHALL assemble the block aggregation witness containing:
- The initial trie state preimages (for TrieDB loading in the aggregation guest).
- Per-chunk metadata: `tx_count`, `post_db_hash`, `post_evm_state`.
- The full final `Cache` state (for verification and trie root computation in the aggregation guest).
- The full final EVM state (for verification and block header construction).
- All chunk proof receipts (appended to the existing `stitched_proofs`).

#### Scenario: aggregation witness is complete
- **WHEN** the prover assembles the aggregation witness
- **THEN** the aggregation guest can verify all chunks and produce a BlockBuildingOutcome without any additional data

#### Scenario: chunk receipts flow through existing stitched_proofs
- **WHEN** chunk receipts are assembled into the witness
- **THEN** they are appended to the `stitched_proofs` vector alongside any block-level stitching receipts

### Requirement: Chunk proving integrates with existing block splitting

Transaction chunk proving SHALL compose with the existing block-level splitting (`max_block_executions`, witness size limits). When both are active:
- Block-level splitting divides the block range into execution-only proof jobs.
- Within each execution-only job, transaction chunking further divides individual blocks.

#### Scenario: block splitting + chunk proving
- **WHEN** `max_block_executions = 2` and `max_txs_per_chunk = 5` with blocks of 20 transactions each
- **THEN** blocks are grouped into execution-only jobs of 2 blocks, and within each block, transactions are chunked into groups of 5

#### Scenario: block splitting without chunk proving
- **WHEN** `max_block_executions = 2` and `max_txs_per_chunk = usize::MAX`
- **THEN** blocks are grouped into execution-only jobs of 2 blocks, proven monolithically (existing behavior)

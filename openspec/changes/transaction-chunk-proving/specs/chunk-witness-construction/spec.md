## ADDED Requirements

### Requirement: Host computes per-transaction tx-body EvmState traces via TracingEvmFactory

The host-side pre-execution SHALL use `TracingEvmFactory` to capture per-transaction `EvmState` (the `HashMap<Address, Account>` from `ResultAndState.state`) during `build_block()`'s ordered transaction-body execution. Each trace entry corresponds to one ordered block transaction and contains all accounts and storage slots that the transaction accessed (read or wrote). Block-level prelude and epilogue state transitions are handled outside this trace sequence.

#### Scenario: trace count matches transaction count
- **WHEN** a block with N transactions is pre-executed on the host
- **THEN** the trace buffer contains exactly N entries

#### Scenario: trace contains original and present values
- **WHEN** a transaction reads storage slot S at address A
- **THEN** the trace entry for that transaction includes `(A, S)` with `original_value` (pre-block trie value) and `present_value` (value after this transaction)

#### Scenario: prelude and epilogue do not create extra trace entries
- **WHEN** the host applies once-per-block prelude and epilogue logic around the transaction body
- **THEN** those state transitions do not add extra entries to the per-transaction trace buffer

### Requirement: Host groups transactions into chunks

When chunk proving is active for a block, the host SHALL group that block's transactions into sequential, non-overlapping chunks based on a positive `max_txs_per_chunk`. The last chunk may have fewer transactions. All transactions in the block MUST be covered by exactly one chunk.

#### Scenario: even division
- **WHEN** a block has 12 transactions and `max_txs_per_chunk = 4`
- **THEN** 3 chunks are created: [tx_0..tx_3], [tx_4..tx_7], [tx_8..tx_11]

#### Scenario: remainder chunk
- **WHEN** a block has 10 transactions and `max_txs_per_chunk = 4`
- **THEN** 3 chunks are created: [tx_0..tx_3], [tx_4..tx_7], [tx_8..tx_9]

#### Scenario: grouping helper can return one full-block range
- **WHEN** the grouping helper is invoked directly for a block with 5 transactions and `max_txs_per_chunk = 100`
- **THEN** it returns 1 range containing all 5 transactions, even though prover dispatch would keep this block on the monolithic path

### Requirement: Chunk witness carries execution inputs in addition to state

For each chunk, the host SHALL include in the witness:
- the ordered transactions belonging to that chunk, in the same order used for `tx_hash`
- the block execution context needed to instantiate the same EVM / executor environment as monolithic execution

This context SHALL include all block-level fields needed by chunk execution, such as the block environment and any `OpBlockExecutionCtx` inputs used by the executor.

#### Scenario: guest can compute tx_hash from the witness
- **WHEN** the chunk guest proves a chunk
- **THEN** it computes `tx_hash` from the transactions carried directly in the witness rather than reconstructing them from global block state

#### Scenario: guest can reconstruct the monolithic execution environment
- **WHEN** the chunk guest initializes the EVM and executor
- **THEN** it does so from the block execution context carried in the witness, matching the corresponding monolithic block execution inputs

### Requirement: Host constructs chunk-start flat state witnesses from boundary snapshots

For each chunk, the host SHALL compute a chunk-start `Cache` snapshot derived from the post-prelude boundary state immediately before that chunk executes. The witness MAY prune addresses and storage slots that the chunk cannot access, but for every included address it SHALL carry the full chunk-start `DbAccount` metadata (`AccountInfo` and `account_state`) in addition to the required storage slots. The algorithm SHALL:
1. Materialize the post-prelude flat state once.
2. Track a boundary state across chunks by applying all prior chunks' account-level and storage-level writes.
3. For each chunk, identify the externally needed addresses, storage slots, contracts, and block hashes for that chunk.
4. For each needed address, load its full chunk-start `DbAccount` snapshot from the current boundary state, including nonce, balance, code hash, existence / cleared / destroyed state, and any other `AccountInfo` fields.
5. For each needed storage slot, use the value from the chunk-start boundary state. If a prior chunk wrote the slot, use the carried-forward value; otherwise, use the post-prelude trie value.
6. Include all contracts (bytecodes) referenced by the chunk's transactions.
7. Include all block_hashes referenced by the chunk's transactions.

#### Scenario: first chunk reads from post-prelude state
- **WHEN** `chunk_0` reads state and no prior chunk exists
- **THEN** the witness includes the chunk-start boundary values from the post-prelude flat state

#### Scenario: later chunk reads prior chunk's write
- **WHEN** chunk_0 writes slot S at address A to value V, and chunk_1 reads slot S at address A
- **THEN** chunk_1's witness includes `(A, S) -> V` (chunk_0's written value, not the pre-block value)

#### Scenario: later chunk sees prior chunk's account metadata changes
- **WHEN** chunk_0 changes an address's nonce, balance, code hash, or existence state, and chunk_1 accesses that address
- **THEN** chunk_1's witness includes the updated chunk-start `DbAccount` snapshot from after chunk_0

#### Scenario: later chunk sees prior chunk's contract creation
- **WHEN** chunk_0 creates a contract at address A and chunk_1 calls or inspects A
- **THEN** chunk_1's witness includes A as an existing account with the created code hash and bytecode from the chunk-start boundary state

#### Scenario: later chunk sees prior chunk's selfdestruct or cleared state
- **WHEN** chunk_0 selfdestructs or clears account A and chunk_1 accesses A
- **THEN** chunk_1's witness includes A's updated `account_state` from the chunk-start boundary state rather than the original trie state

#### Scenario: intra-chunk write satisfies later read
- **WHEN** tx_0 in chunk_1 writes slot S, and tx_2 in chunk_1 reads slot S
- **THEN** slot S is NOT required in chunk_1's external witness (the write within the chunk satisfies the read)

#### Scenario: witness completeness
- **WHEN** a chunk witness is constructed and provided to the chunk guest
- **THEN** no PanicDB fallback is triggered during execution (all required account metadata, storage values, contracts, and block hashes are present)

### Requirement: Chunk witness uses rkyv-serializable mirror types

The chunk witness `Cache` state SHALL be serialized using rkyv-compatible mirror types (`SerializableCache`, `SerializableDbAccount`, etc.) since revm's native `Cache`, `DbAccount`, `AccountInfo`, and `Bytecode` types support serde but not rkyv. Conversion between mirror types and native revm types SHALL be lossless and round-trip tested.

#### Scenario: round-trip serialization preserves state
- **WHEN** a `Cache` is converted to `SerializableCache`, serialized with rkyv, deserialized, and converted back to `Cache`
- **THEN** the resulting `Cache` is logically identical to the original (same accounts, storage, contracts, block_hashes)

#### Scenario: canonical hash is preserved across serialization
- **WHEN** a `Cache` is round-tripped through `SerializableCache` and back
- **THEN** the canonical hash of the original and the round-tripped `Cache` are identical

### Requirement: Host constructs EVM state accumulators per chunk

For each chunk, the host SHALL compute the pre-chunk and post-chunk EVM state accumulators (cumulative_gas_used, da_footprint_used, blob_gas_used, logs_bloom, receipts) from the per-transaction traces, explicit per-transaction metadata for fields not derivable from `EvmState` alone, and the block execution result.

The per-transaction metadata SHALL include, at minimum:
- exact `BLOCKHASH` reads performed by the tx body
- per-transaction `da_footprint_used` delta
- per-transaction `blob_gas_used` delta

#### Scenario: first chunk starts with zero accumulators
- **WHEN** chunk_0's EVM state is constructed
- **THEN** pre-chunk values are: gas=0, da_footprint=0, blob_gas=0, bloom=empty, receipts=[]

#### Scenario: accumulator continuity between chunks
- **WHEN** chunk_0 ends with cumulative_gas_used=500000
- **THEN** chunk_1's pre-chunk cumulative_gas_used is 500000

#### Scenario: absent account is materialized explicitly
- **WHEN** a chunk touches an address that is absent at the chunk-start boundary state
- **THEN** the witness includes that address as an explicit `NotExisting` account entry rather than omitting it

#### Scenario: first chunk includes code for a pre-existing called contract
- **WHEN** chunk_0 calls a pre-existing contract whose bytecode was not yet populated in the post-prelude cache's contract map
- **THEN** the witness includes that contract bytecode using tx-body execution metadata from the tracing run

#### Scenario: tx-body block hash reads are witness inputs
- **WHEN** a transaction performs a `BLOCKHASH` read that was not already present in the post-prelude cache
- **THEN** the host provides that block hash in the chunk witness using explicit per-transaction block-hash metadata so the guest does not fall through to `PanicDB`

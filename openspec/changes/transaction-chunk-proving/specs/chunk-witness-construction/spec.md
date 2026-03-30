## ADDED Requirements

### Requirement: Host computes per-transaction EvmState traces via TracingEvmFactory

The host-side pre-execution SHALL use `TracingEvmFactory` to capture per-transaction `EvmState` (the `HashMap<Address, Account>` from `ResultAndState.state`) during `build_block()`. Each trace entry corresponds to one transaction and contains all accounts and storage slots that the transaction accessed (read or wrote).

#### Scenario: trace count matches transaction count
- **WHEN** a block with N transactions is pre-executed on the host
- **THEN** the trace buffer contains exactly N entries

#### Scenario: trace contains original and present values
- **WHEN** a transaction reads storage slot S at address A
- **THEN** the trace entry for that transaction includes `(A, S)` with `original_value` (pre-block trie value) and `present_value` (value after this transaction)

### Requirement: Host groups transactions into chunks

The host SHALL group a block's transactions into sequential, non-overlapping chunks based on `max_txs_per_chunk`. The last chunk may have fewer transactions. All transactions in the block MUST be covered by exactly one chunk.

#### Scenario: even division
- **WHEN** a block has 12 transactions and `max_txs_per_chunk = 4`
- **THEN** 3 chunks are created: [tx_0..tx_3], [tx_4..tx_7], [tx_8..tx_11]

#### Scenario: remainder chunk
- **WHEN** a block has 10 transactions and `max_txs_per_chunk = 4`
- **THEN** 3 chunks are created: [tx_0..tx_3], [tx_4..tx_7], [tx_8..tx_9]

#### Scenario: single chunk when max >= block size
- **WHEN** a block has 5 transactions and `max_txs_per_chunk = 100`
- **THEN** 1 chunk is created containing all 5 transactions (effectively monolithic)

### Requirement: Host constructs minimal flat state witness per chunk

For each chunk, the host SHALL compute the minimal `Cache` (accounts + storage + contracts + block_hashes) that the chunk's transactions will access. The algorithm SHALL:
1. Track cumulative writes from all prior chunks.
2. For each transaction in the chunk, identify external reads: (addr, slot) pairs that are not written by a prior transaction within the same chunk.
3. For each external read: if a prior chunk wrote to that (addr, slot), use the prior chunk's written value. Otherwise, use the `original_value` from the trace (pre-block trie value).
4. Include all contracts (bytecodes) referenced by the chunk's transactions.
5. Include all block_hashes referenced by the chunk's transactions.

#### Scenario: first chunk reads from pre-block state
- **WHEN** chunk_0 reads storage slot S at address A and no prior chunk exists
- **THEN** the witness includes `(A, S) -> original_value` from the pre-block trie

#### Scenario: later chunk reads prior chunk's write
- **WHEN** chunk_0 writes slot S at address A to value V, and chunk_1 reads slot S at address A
- **THEN** chunk_1's witness includes `(A, S) -> V` (chunk_0's written value, not the pre-block value)

#### Scenario: intra-chunk write satisfies later read
- **WHEN** tx_0 in chunk_1 writes slot S, and tx_2 in chunk_1 reads slot S
- **THEN** slot S is NOT required in chunk_1's external witness (the write within the chunk satisfies the read)

#### Scenario: witness completeness
- **WHEN** a chunk witness is constructed and provided to the chunk guest
- **THEN** no PanicDB fallback is triggered during execution (all required state is present)

### Requirement: Host constructs EVM state accumulators per chunk

For each chunk, the host SHALL compute the pre-chunk and post-chunk EVM state accumulators (cumulative_gas_used, da_footprint_used, blob_gas_used, logs_bloom, receipts) from the per-transaction traces and the block execution result.

#### Scenario: first chunk starts with zero accumulators
- **WHEN** chunk_0's EVM state is constructed
- **THEN** pre-chunk values are: gas=0, da_footprint=0, blob_gas=0, bloom=empty, receipts=[]

#### Scenario: accumulator continuity between chunks
- **WHEN** chunk_0 ends with cumulative_gas_used=500000
- **THEN** chunk_1's pre-chunk cumulative_gas_used is 500000

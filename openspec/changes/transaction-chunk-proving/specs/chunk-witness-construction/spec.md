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

### Requirement: Host constructs EVM state accumulators per chunk

For each chunk, the host SHALL compute the pre-chunk and post-chunk EVM state accumulators (cumulative_gas_used, da_footprint_used, blob_gas_used, logs_bloom, receipts) from the per-transaction traces and the block execution result.

#### Scenario: first chunk starts with zero accumulators
- **WHEN** chunk_0's EVM state is constructed
- **THEN** pre-chunk values are: gas=0, da_footprint=0, blob_gas=0, bloom=empty, receipts=[]

#### Scenario: accumulator continuity between chunks
- **WHEN** chunk_0 ends with cumulative_gas_used=500000
- **THEN** chunk_1's pre-chunk cumulative_gas_used is 500000

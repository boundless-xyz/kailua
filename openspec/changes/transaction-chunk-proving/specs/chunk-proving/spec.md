## ADDED Requirements

### Requirement: Chunk guest executes only the transaction body against CacheDB with PanicDB fallback

The chunk proving mode SHALL execute only a subset of a block's ordered transaction body against a `CacheDB<PanicDB>` (or equivalent in-memory DB backed by a panicking fallback). The witness cache for `chunk_0` represents the post-prelude block state, and the final chunk stops at the post-last-transaction, pre-epilogue state. All required state MUST be pre-loaded in the cache. If any transaction reads state not present in the cache, the guest SHALL panic.

#### Scenario: all state present, execution succeeds
- **WHEN** the chunk witness contains all accounts and storage slots that the chunk's transactions will access
- **THEN** all transactions execute successfully and the chunk proof completes

#### Scenario: missing state causes panic
- **WHEN** a transaction in the chunk attempts to read a storage slot not present in the CacheDB cache
- **THEN** the PanicDB fallback is triggered and the guest panics

#### Scenario: system transactions handled like regular transactions
- **WHEN** a chunk includes system deposit transactions (L1 info deposit, etc.)
- **THEN** they are executed in order like any other transaction, with their state provided in the cache

#### Scenario: chunk_0 starts from the post-prelude state
- **WHEN** the first chunk begins execution
- **THEN** the witness cache already includes any once-per-block prelude state changes, and the chunk guest does not replay those changes

#### Scenario: final chunk stops before epilogue
- **WHEN** the last chunk completes execution
- **THEN** its `post_db_hash` and `post_evm_state_hash` describe the post-last-transaction, pre-epilogue state, and the chunk guest does not apply the epilogue

### Requirement: Chunk guest computes pre/post memory DB hashes

The chunk guest SHALL compute the SHA256 hash of the `Cache` structure before executing any transactions (`pre_db_hash`) and after executing all transactions (`post_db_hash`), using the canonical encoding defined in memory-db-hashing.

#### Scenario: pre_db_hash reflects loaded chunk-start state
- **WHEN** the chunk guest loads the memory DB from the witness
- **THEN** `pre_db_hash = SHA256(canonical_encode(cache))` before any transaction executes

#### Scenario: post_db_hash reflects modified state
- **WHEN** all chunk transactions have executed and committed their state changes
- **THEN** `post_db_hash = SHA256(canonical_encode(cache))` reflects the modified cache

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

### Requirement: Chunk guest computes tx_hash

The chunk guest SHALL compute `tx_hash = SHA256(canonical_encode(transactions))` where `transactions` is the ordered list of transactions in the chunk.

#### Scenario: tx_hash is deterministic
- **WHEN** the same transactions in the same order are provided
- **THEN** `tx_hash` is identical across invocations

### Requirement: Chunk guest emits standard ProofJournal with chunk sentinel

The chunk guest SHALL emit a `ProofJournal` with:
- `payout_recipient`: from witness
- `precondition_hash`: digest of `Precondition::default().chunk(chunk_trace)` where `chunk_trace = SHA256(tx_hash || pre_db_hash || post_db_hash || pre_evm_state_hash || post_evm_state_hash)`
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

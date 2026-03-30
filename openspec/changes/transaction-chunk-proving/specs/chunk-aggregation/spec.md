## ADDED Requirements

### Requirement: Block aggregation loads initial state from trie into memory DB

The block aggregation mode SHALL load the required state from the state trie (via TrieDB/preimage oracles, as done in current block execution) and populate a `Cache` structure. The SHA256 of this initial cache is `initial_db_hash`. The initial EVM state accumulators SHALL be zeroed, producing `initial_evm_hash`.

#### Scenario: initial state is Merkle-verified
- **WHEN** the block aggregation proof loads state from the trie
- **THEN** each account and storage value is verified against the state trie's Merkle proofs (existing TrieDB behavior)

#### Scenario: initial_db_hash matches chunk_0's pre_db_hash
- **WHEN** chunk_0 was proven with `pre_db_hash = H`
- **THEN** the block aggregation computes `initial_db_hash == H`

### Requirement: Block aggregation verifies chunk proof chain

For each chunk `i` in order (0 to N-1), the block aggregation proof SHALL:
1. Load `tx_count_i`, `post_db_hash_i`, and `post_evm_state_i` from the witness.
2. Compute `tx_hash_i` from the block's transactions at the current offset.
3. Compute `pre_evm_state_hash_i` from `current_evm_state`.
4. Compute `chunk_trace_i = SHA256(tx_hash_i || current_db_hash || post_db_hash_i || pre_evm_state_hash_i || post_evm_hash_i)`.
5. Construct the expected `ProofJournal` for chunk `i` using `chunk_trace_i`.
6. Verify the chunk's receipt via `verify_stitching_journal()`.
7. Advance: `current_db_hash = post_db_hash_i`, `current_evm_state = post_evm_state_i`, `offset += tx_count_i`.

#### Scenario: all chunks verified in order
- **WHEN** a block has 3 chunks with valid receipts
- **THEN** chunks 0, 1, 2 are verified in sequence, each advancing the current hash state

#### Scenario: chunk receipt mismatch causes failure
- **WHEN** chunk `i`'s receipt does not match the expected ProofJournal
- **THEN** `verify_stitching_journal()` fails (panics in zkVM)

#### Scenario: memory DB hash continuity
- **WHEN** chunk `i` is verified with `post_db_hash_i`
- **THEN** chunk `i+1`'s expected `pre_db_hash` is `post_db_hash_i`

#### Scenario: EVM state hash continuity
- **WHEN** chunk `i` is verified with `post_evm_hash_i`
- **THEN** chunk `i+1`'s expected `pre_evm_state_hash` is `post_evm_hash_i`

### Requirement: Block aggregation verifies final state and computes trie root

After all chunks are verified, the block aggregation proof SHALL:
1. Load the full final `Cache` from the witness.
2. Verify `SHA256(final_cache) == current_db_hash` (matches last chunk's post_db_hash).
3. Load the full final EVM state from the witness.
4. Verify `SHA256(final_evm_state) == current_evm_hash` (matches last chunk's post_evm_hash).
5. Diff `final_cache` against `initial_cache` to produce a `BundleState`.
6. Compute `state_root = trie_db.state_root(&bundle)` — the single trie computation for the block.

#### Scenario: final cache hash mismatch causes failure
- **WHEN** `SHA256(final_cache) != current_db_hash`
- **THEN** the block aggregation proof fails

#### Scenario: state root matches monolithic execution
- **WHEN** a block is proven via chunking
- **THEN** the computed `state_root` is identical to what monolithic execution would produce

### Requirement: Block aggregation produces standard BlockBuildingOutcome

The block aggregation proof SHALL construct a `BlockBuildingOutcome` (sealed header + execution result) using:
- `state_root` from the trie computation
- `receipts_root` from `compute_receipts_root()` using the final EVM state's receipts
- `gas_used` from the final EVM state's `cumulative_gas_used`
- `logs_bloom` from the final EVM state
- `blob_gas_used` from the final EVM state
- All other header fields from the payload attributes (same as `seal_block()`)

The resulting `BlockBuildingOutcome` SHALL be identical to what monolithic execution produces.

#### Scenario: block-level ProofJournal is standard
- **WHEN** the block aggregation emits its ProofJournal
- **THEN** it is a standard block-level ProofJournal (not a chunk journal) with a real `l1_head` or `0x00..00` (execution-only), identical to monolithic execution

#### Scenario: transparent to block stitching
- **WHEN** a chunk-aggregated block is stitched with other blocks via `stitch_executions()` or `stitch_boot_info()`
- **THEN** the stitching succeeds identically to monolithic blocks — chunking is invisible at the stitching layer

### Requirement: Block aggregation integrates with CachedExecutor

The `CachedExecutor` SHALL support a chunk aggregation path alongside the existing cache-hit and direct-execution paths. When chunk data is available for a block, the executor SHALL perform chunk verification and aggregation instead of monolithic execution.

#### Scenario: executor selects chunk path when data is present
- **WHEN** `execute_payload(attributes)` is called and chunk data exists for the current block
- **THEN** the executor runs chunk aggregation (not monolithic execution)

#### Scenario: executor falls back to monolithic when no chunk data
- **WHEN** `execute_payload(attributes)` is called and no chunk data exists
- **THEN** the executor follows the existing cache-hit or direct-execution path (unchanged)

#### Scenario: executor still supports block cache hit
- **WHEN** `execute_payload(attributes)` is called and the block is in the cache
- **THEN** the cache hit takes precedence over chunk aggregation (existing behavior, unchanged)

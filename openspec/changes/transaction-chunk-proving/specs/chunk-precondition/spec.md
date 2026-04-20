## ADDED Requirements

### Requirement: Precondition struct has a chunk_trace field

The `Precondition` struct SHALL have a fifth field `pub chunk_trace: B256` alongside the existing `proposal_blobs`, `execution_trace`, `derivation_cache`, and `derivation_trace` fields. The field SHALL support the same derive macros and rkyv serialization as existing fields.

#### Scenario: default value is zero
- **WHEN** `Precondition::default()` is constructed
- **THEN** `chunk_trace` is `B256::ZERO`

#### Scenario: builder method sets chunk_trace
- **WHEN** `Precondition::default().chunk(some_hash)` is called
- **THEN** the returned Precondition has `chunk_trace == some_hash` and all other fields remain `B256::ZERO`

### Requirement: Digestible dispatches chunk_trace as exclusive mode

The `Digestible` implementation for `Precondition` SHALL treat `chunk_trace` as a third exclusive mode. When `chunk_trace` is non-zero, all other fields MUST be zero (asserted). The digest SHALL be `Digest::from_bytes(chunk_trace.0)`.

#### Scenario: chunk-only precondition digest
- **WHEN** `Precondition { chunk_trace: H, execution_trace: ZERO, proposal_blobs: ZERO, derivation_cache: ZERO, derivation_trace: ZERO }.digest()` is called
- **THEN** the result is `Digest::from_bytes(H.0)`

#### Scenario: chunk_trace with non-zero execution_trace panics
- **WHEN** `Precondition { chunk_trace: non_zero, execution_trace: non_zero, .. }.digest()` is called
- **THEN** the call panics due to assertion failure

#### Scenario: existing execution-only mode unchanged
- **WHEN** `Precondition { execution_trace: H, chunk_trace: ZERO, .. }.digest()` is called
- **THEN** the result is identical to the current behavior: `Digest::from_bytes(H.0)`

#### Scenario: existing derivation/proposal mode unchanged
- **WHEN** `Precondition { execution_trace: ZERO, chunk_trace: ZERO, derivation_cache: A, derivation_trace: B, proposal_blobs: C }.digest()` is called
- **THEN** the result is identical to the current behavior: `combine(merge(A, B), C)`

### Requirement: chunk_trace is computed from seven hashes

The `chunk_trace` value for a chunk proof SHALL be `SHA256(tx_hash || pre_db_hash || post_db_hash || pre_evm_state_hash || post_evm_state_hash || results_hash || block_ctx_hash)` where:
- `tx_hash`: SHA256 of the canonical encoding of the chunk's transaction list
- `pre_db_hash`: SHA256 of the canonical encoding of the memory DB before chunk execution
- `post_db_hash`: SHA256 of the canonical encoding of the memory DB after chunk execution
- `pre_evm_state_hash`: SHA256 of the canonical encoding of the EVM state accumulators before chunk execution
- `post_evm_state_hash`: SHA256 of the canonical encoding of the EVM state accumulators after chunk execution
- `results_hash`: SHA256 of the canonical encoding of the per-transaction `ResultAndState` trace captured during chunk execution (see `hash_results` — excludes transient fields `transaction_id`, `is_cold`, `original_info`, `AccountInfo.code`)
- `block_ctx_hash`: SHA256 of the canonical encoding of the `BlockEnv` and `OpBlockExecutionCtx` under which the chunk guest executed (see `hash_block_ctx` — covers `number`, `beneficiary`, `timestamp`, `gas_limit`, `basefee`, `difficulty`, `prevrandao`, `blob_excess_gas_and_price`, `parent_hash`, `parent_beacon_block_root`, `extra_data`)

The `results_hash` component binds the chunk proof to the exact per-transaction execution trace, not merely the pre→post state endpoints. This is what enables the aggregation proof to safely replay `Chunk.results` verbatim without re-execution while still authenticating each transition.

The `block_ctx_hash` component binds the chunk proof to the exact block execution context that produced those results — without it, an adversary could generate a valid chunk proof under a forged context (different timestamp, basefee, prevrandao, coinbase, blob pricing, parent_hash, etc.) and have aggregation accept env-sensitive opcode results (BASEFEE / PREVRANDAO / NUMBER / COINBASE / TIMESTAMP / BLOBBASEFEE / BLOCKHASH / EIP-4788 beacon root / EIP-2935 ring / Holocene-Jovian EIP-1559 params) for the real block. The aggregation side ALSO verifies each chunk's carried `block_env` / `op_block_ctx` against the derivation pipeline's actual block header, so the chunk must have executed under both the claimed context (binding via `block_ctx_hash` in `chunk_trace`) AND the derivation-produced context (cross-check in `verify_block_chunks`).

#### Scenario: deterministic computation
- **WHEN** the same seven input hashes are provided in the same order
- **THEN** the resulting `chunk_trace` is identical across invocations

#### Scenario: any input change produces a different chunk_trace
- **WHEN** any one of the seven input hashes differs
- **THEN** the resulting `chunk_trace` differs (collision resistance of SHA256)

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

### Requirement: chunk_trace is computed from five hashes

The `chunk_trace` value for a chunk proof SHALL be `SHA256(tx_hash || pre_db_hash || post_db_hash || pre_evm_state_hash || post_evm_state_hash)` where:
- `tx_hash`: SHA256 of the canonical encoding of the chunk's transaction list
- `pre_db_hash`: SHA256 of the canonical encoding of the memory DB before chunk execution
- `post_db_hash`: SHA256 of the canonical encoding of the memory DB after chunk execution
- `pre_evm_state_hash`: SHA256 of the canonical encoding of the EVM state accumulators before chunk execution
- `post_evm_state_hash`: SHA256 of the canonical encoding of the EVM state accumulators after chunk execution

#### Scenario: deterministic computation
- **WHEN** the same five input hashes are provided in the same order
- **THEN** the resulting `chunk_trace` is identical across invocations

#### Scenario: any input change produces a different chunk_trace
- **WHEN** any one of the five input hashes differs
- **THEN** the resulting `chunk_trace` differs (collision resistance of SHA256)

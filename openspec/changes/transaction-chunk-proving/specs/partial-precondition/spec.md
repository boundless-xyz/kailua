## ADDED Requirements

### Requirement: Precondition struct has a `partial_executions` field

The `Precondition` struct SHALL have a fifth field `pub partial_executions: B256` alongside the existing `proposal_blobs`, `execution_trace`, `derivation_cache`, and `derivation_trace` fields. It SHALL use the same derive macros and rkyv `B256Def` wrapper as the other fields. A `.partial(trace: B256) -> Self` builder method SHALL set this field.

#### Scenario: default value is zero
- **WHEN** `Precondition::default()` is constructed
- **THEN** `partial_executions` is `B256::ZERO`

#### Scenario: builder method sets partial_executions
- **WHEN** `Precondition::default().partial(some_hash)` is called
- **THEN** the returned Precondition has `partial_executions == some_hash` and all other fields remain `B256::ZERO`

### Requirement: Digestible dispatches `partial_executions` as an exclusive mode

The `Digestible` implementation for `Precondition` SHALL treat `partial_executions` as a third exclusive mode. When `partial_executions` is non-zero, all other fields MUST be zero (asserted). The digest SHALL be `Digest::from_bytes(partial_executions.0)`.

#### Scenario: partial-only precondition digest
- **WHEN** `Precondition { partial_executions: H, all_other_fields: ZERO }.digest()` is called
- **THEN** the result is `Digest::from_bytes(H.0)`

#### Scenario: partial_executions with any other non-zero field panics
- **WHEN** `Precondition { partial_executions: non_zero, <any_other_field>: non_zero, .. }.digest()` is called
- **THEN** the call panics due to assertion failure (covered for `execution_trace`, `proposal_blobs`, `derivation_cache`, and `derivation_trace`)

#### Scenario: existing execution-only mode unchanged
- **WHEN** `Precondition { execution_trace: H, partial_executions: ZERO, .. }.digest()` is called
- **THEN** the result is identical to the current behavior: `Digest::from_bytes(H.0)`

#### Scenario: existing derivation/proposal mode unchanged
- **WHEN** `Precondition { execution_trace: ZERO, partial_executions: ZERO, derivation_cache: A, derivation_trace: B, proposal_blobs: C }.digest()` is called
- **THEN** the result is identical to the current behavior: `combine(merge(A, B), C)`

### Requirement: `partial_executions` is the SHA256 of `results_hash || block_ctx_hash`

The `partial_executions` value for a partial proof SHALL be computed by `compute_pe_trace(results_hash, block_ctx_hash)` — defined as `SHA256(results_hash || block_ctx_hash)` — where:

- `results_hash = hash_results(tx_hashes, results)` — SHA256 over the per-transaction `(tx_hash, ResultAndState<OpHaltReason>)` pairs the partial guest captured. The canonical encoding includes each `ResultAndState`'s `ExecutionResult` (variant tag, gas values, logs, output bytes) AND its `EvmState` (sorted per-address, per-storage-slot: `original_info` + `info` + `status` bitflags + storage slots' `original_value` / `present_value`). Transient fields (`transaction_id`, `is_cold`, `AccountInfo.code`) are excluded so the same logical trace produces the same hash regardless of execution context.

- `block_ctx_hash = hash_block_ctx(block_env, op_block_ctx)` — SHA256 over every `BlockEnv` field (number, beneficiary, timestamp, gas_limit, basefee, difficulty, prevrandao, blob_excess_gas_and_price) and every `OpBlockExecutionCtx` field (parent_hash, parent_beacon_block_root, extra_data).

The `results_hash` binding forces the partial proof to authenticate the exact per-transaction trace (results, state diffs, AND pre-tx `original_info`/`original_value`). The `block_ctx_hash` binding forces the partial proof to have been generated under the exact block execution context — preventing an adversary from producing a valid proof under a forged context (different timestamp, basefee, prevrandao, coinbase, blob pricing, parent_hash, extra_data, etc.) that yields different env-sensitive opcode results.

No separate pre/post memory-DB hashes, EVM-accumulator hashes, or tx-list hash contribute to the commitment: continuity across multiple partials within a block is enforced by `CachedEvm`'s per-tx prestate authentication at aggregation time, not by a precondition-level hash chain.

#### Scenario: deterministic computation
- **WHEN** the same `(results_hash, block_ctx_hash)` pair is provided
- **THEN** the resulting `partial_executions` is identical across invocations

#### Scenario: any input change produces a different trace
- **WHEN** any byte of `results_hash` or `block_ctx_hash` differs
- **THEN** the resulting `partial_executions` differs (collision-resistance of SHA256)

#### Scenario: `original_info` contributes to `results_hash`
- **WHEN** two traces agree on every post-state field but differ in any account's pre-tx `original_info` (nonce, balance, code_hash)
- **THEN** their `results_hash` values differ — the partial proof authenticates the chunk's first-load view of state, which `CachedEvm` later cross-checks against the live aggregation DB

#### Scenario: transient fields do not contribute
- **WHEN** two traces differ only in `transaction_id` or `is_cold` across accounts / storage slots
- **THEN** their `results_hash` values are identical (transient fields are excluded from the canonical encoding)

#### Scenario: `block_ctx_hash` changes on any context field change
- **WHEN** any field of `BlockEnv` (BASEFEE / PREVRANDAO / NUMBER / COINBASE / TIMESTAMP / gas_limit / difficulty / BLOBBASEFEE) or `OpBlockExecutionCtx` (parent_hash / parent_beacon_block_root / extra_data) changes
- **THEN** `block_ctx_hash` differs, `partial_executions` differs, and the partial proof's `ProofJournal` identity changes — causing `env::verify()` to reject a substituted context on the aggregation side

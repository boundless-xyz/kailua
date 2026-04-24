## ADDED Requirements

### Requirement: Host captures per-transaction `(tx_hash, ResultAndState)` via `CachedEvmFactory`'s trace collector

During host-side pre-execution the `CachedEvmFactory` (in `kailua-kona` at `crates/kona/src/evm.rs`, shared between host pre-execution and guest aggregation) SHALL be constructed with `Some(Arc::new(Mutex::new(Vec::new())))` as its `block_traces` argument. The factory inserts a fresh per-EVM `Vec` via `push_trace_slot` before each EVM creation; every fall-through `transact_raw` (i.e., any transaction whose hash is not already served from a seeded partial) appends its `(keccak256(enveloped_tx), ResultAndState)` pair to that `Vec`. Block-level system calls (`transact_system_call`) do NOT append.

The host SHALL drain the buffer after `run_core_client` returns via `take_all_block_traces()`, yielding one inner `Vec<(B256, ResultAndState)>` per L2 block in the execution range.

#### Scenario: trace count equals tx-body count
- **WHEN** a block with `N` tx-body transactions is executed on the host with no seeded partials
- **THEN** the per-EVM capture `Vec` contains exactly `N` entries in execution order

#### Scenario: system calls are not captured
- **WHEN** block-level prelude / epilogue work (beacon-root contract call, block-hash ring buffer update, canyon deployer, post-block balance increments) runs
- **THEN** those calls delegate through `CachedEvm::transact_system_call` and append nothing to the trace collector

#### Scenario: seeded partials do not duplicate captures
- **WHEN** a block's transactions are partly served from seeded `PartialExecution` entries and partly executed via the inner EVM
- **THEN** only fall-through executions append to the trace collector (served cached entries are consumed from the seeded partial)

### Requirement: Host assembles `PartialExecution` entries from collected traces

After `run_core_client` returns, the host SHALL iterate (via `recover_collected_partials` in `kailua-kona` at `crates/kona/src/client/core.rs` and `build_single_partial_for_block` in `crates/kona/src/executor.rs`) the drained collector in lock-step with the `Execution` cache and invoke `build_single_partial_for_block(execution, captured_traces, parent_header, spec_id)` for each block. This helper SHALL:

1. Unzip `captured_traces: Vec<(B256, ResultAndState)>` into `(tx_hashes, results)`.
2. Reconstruct a `BlockEnv` from `execution.artifacts.header` fields (number, beneficiary, timestamp, gas_limit, basefee, difficulty, prevrandao = Some(mix_hash)) with `blob_excess_gas_and_price = expected_blob_excess_gas_and_price(parent_header, spec_id)`.
3. Reconstruct an `OpBlockExecutionCtx { parent_hash, parent_beacon_block_root, extra_data }` from the same header.
4. Return `PartialExecution { tx_hashes, results, block_env, op_block_ctx }`.

`parent_header` is threaded through the outer loop so each block's context correctly references its parent; the first block uses the safe-head header, subsequent blocks use the previous block's sealed header.

This approach emits exactly ONE `PartialExecution` covering the entire block. Tests that split one block into multiple partials (e.g. `split_partials`) build sub-partials by slicing `tx_hashes` / `results` while cloning the shared `block_env` / `op_block_ctx`.

#### Scenario: one captured block yields one full-block partial by default
- **WHEN** a block with `N` tx-body transactions is executed and its traces recovered
- **THEN** `recover_collected_partials` produces a `Vec<Vec<PartialExecution>>` with one outer entry containing one `PartialExecution` whose `tx_hashes.len() == N`

#### Scenario: `block_env` mirrors the sealed header's fields
- **WHEN** `build_single_partial_for_block` runs against a sealed block's header
- **THEN** the returned `block_env` fields match the header exactly (and `blob_excess_gas_and_price` is derived from `parent_header` + `spec_id` via the shared helper that mirrors kona's `prepare_block_env`)

#### Scenario: test helper `split_partials` preserves block_env / op_block_ctx
- **WHEN** `split_partials` turns a block-wide `PartialExecution` into one-tx-per-partial chunks
- **THEN** every produced sub-partial carries a clone of the original `block_env` and `op_block_ctx`

### Requirement: `PartialExecutionWitness` carries the inputs for a single partial proof

The system SHALL define a `PartialExecutionWitness` struct in `kailua-kona` at `crates/kona/src/evm.rs` with four fields:

- `transactions: Vec<Vec<u8>>` — the ordered, RLP-encoded EIP-2718 transactions to re-execute.
- `cache: CacheState` — the pre-state the guest seeds into `State::builder().with_cached_prestate(cache)`. Serialized via `CacheStateRkyv`.
- `block_env: BlockEnv` — the block environment (serialized via `BlockEnvRkyv`).
- `op_block_ctx: OpBlockExecutionCtx` — the OP block context (serialized via `OpBlockExecutionCtxRkyv`).

A `PartialExecutionWitness::from_preflight(partial: PartialExecution, execution: &Execution) -> Self` constructor SHALL:

1. Collect the concrete transaction bytes from `execution.get_transactions(&partial.tx_hashes)` — a lookup by hash against the `Execution.attributes.transactions` list.
2. Build a `CacheState` by folding `partial.results` through `cache_results(...)` — which collapses every per-tx `EvmState` into a `CacheState` by inserting each account's `original_info` + (for initial loads) its storage `original_value` into the cache. Bytecode carried in `AccountInfo.code` is lifted into `cache.contracts` keyed by `code_hash`.
3. Carry `partial.block_env` and `partial.op_block_ctx` through unchanged.

Note: the witness does NOT need to ship an `EvmAccumulatorState`, a `tx_count`, a block number, a chunk index, per-tx metadata (da_footprint / blob_gas deltas / block-hash reads), or positional block-level commitments (`agreed_l2_output_root`, `config_hash`, etc.) — those either live in the `BootInfo` (which is always shipped) or are reconstructable at proof time from the four fields above.

#### Scenario: witness ingestion produces a valid partial prestate
- **WHEN** a `PartialExecution` is converted via `from_preflight`
- **THEN** the resulting witness contains (a) transactions resolvable from `execution.attributes.transactions`, (b) a `CacheState` covering every address / slot / contract that any tx in the partial first-loaded, (c) the original `block_env` / `op_block_ctx`

#### Scenario: `cache_results` deduplicates first-load entries
- **WHEN** multiple transactions in the same partial touch the same account or storage slot
- **THEN** only the first-load `original_info` / `original_value` is inserted into the resulting `CacheState` (later overwrites are ignored)

#### Scenario: `cache_results` lifts inline bytecode into the contracts map
- **WHEN** a transaction's account `original_info.code` is `Some(bytecode)`
- **THEN** `cache_results` inserts `(code_hash, bytecode)` into `cache.contracts` exactly once per code hash

### Requirement: `Witness` carries optional `pe_witness` and positional `partial_executions`

The `Witness` struct in `kailua-kona` at `crates/kona/src/witness.rs` SHALL carry two new fields:

- `pe_witness: Option<PartialExecutionWitness>` — populated only when the host is producing a partial proof (`boot.l1_head == 0xFF..FF`).
- `partial_executions: Vec<Vec<PartialExecution>>` — positional per-block partials to seed into `CachedEvmFactory` for the outer run. Empty by default.

These fields SHALL thread through `run_stateless_client()` → `run_stitching_client()` → `run_core_client()` as explicit parameters alongside the existing `stitched_executions`, `derivation_cache`, etc.

#### Scenario: default witness has no partial data
- **WHEN** `Witness::default()` is constructed
- **THEN** `pe_witness == None` and `partial_executions == Vec::new()`

#### Scenario: rkyv round-trip preserves partial fields
- **WHEN** a populated `Witness` with `pe_witness = Some(...)` and non-empty `partial_executions` is serialized and deserialized
- **THEN** both fields round-trip byte-for-byte

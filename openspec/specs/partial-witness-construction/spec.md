### Requirement: Host captures per-transaction `PartialExecutionTrace` via `CachedEvmFactory`'s trace collector

During host-side pre-execution the `CachedEvmFactory` (in `kailua-kona` at `crates/kona/src/evm/cached.rs`, shared between host pre-execution and guest aggregation) SHALL be constructed with `Some(Arc::new(Mutex::new(Vec::new())))` as its `block_traces` argument. The factory inserts a fresh per-EVM `Vec` via `push_trace_slot` before each EVM creation; every fall-through `transact_raw` (i.e., any transaction whose hash is not already served from a seeded partial) captures its `expected_state` snapshot BEFORE the inner-EVM call, then on `Ok` appends `PartialExecutionTrace { tx_hash: keccak256(enveloped_tx), result: PartialResultAndState::from(r.clone()), expected_state }` to that `Vec`. Block-level system calls (`transact_system_call`) do NOT append.

The host SHALL drain the buffer after `run_core_client` returns via `take_all_block_traces()`, yielding one inner `Vec<PartialExecutionTrace>` per L2 block in the execution range.

#### Scenario: trace count equals tx-body count
- **WHEN** a block with `N` tx-body transactions is executed on the host with no seeded partials
- **THEN** the per-EVM capture `Vec` contains exactly `N` entries in execution order

#### Scenario: system calls are not captured
- **WHEN** block-level prelude / epilogue work (beacon-root contract call, block-hash ring buffer update, canyon deployer, post-block balance increments) runs
- **THEN** those calls delegate through `CachedEvm::transact_system_call` and append nothing to the trace collector

#### Scenario: seeded partials do not duplicate captures
- **WHEN** a block's transactions are partly served from seeded `PartialExecution` entries and partly executed via the inner EVM
- **THEN** only fall-through executions append to the trace collector (served cached entries are consumed from the seeded partial)

#### Scenario: each `PartialExecutionTrace` carries the pre-tx `expected_state` snapshot
- **WHEN** a fall-through transaction is captured at trace position `i` of a block
- **THEN** `traces[i].expected_state` equals `capture_required_expected_state(db)` evaluated against the live DB *before* tx `i` was executed (so the snapshot reflects the state of L1Block slots as of the partial's first tx, advancing tx-by-tx as commits land)

### Requirement: Host assembles `PartialExecution` entries from collected traces

After `run_core_client` returns, the host SHALL iterate (via `recover_collected_partials` in `kailua-kona` at `crates/kona/src/client/core.rs` and `build_single_partial_for_block` in `crates/kona/src/executor.rs`) the drained collector in lock-step with the `Execution` cache and invoke `build_single_partial_for_block(execution, captured_traces, parent_header, spec_id)` for each block. This helper SHALL:

1. Take the first trace's `expected_state` as the partial's starting snapshot (the partial-start snapshot equals the snapshot captured before tx 0).
2. Unzip `captured_traces: Vec<PartialExecutionTrace>` into `(tx_hashes, results)`.
3. Reconstruct a `BlockEnv` from `execution.artifacts.header` fields (number, beneficiary, timestamp, gas_limit, basefee, difficulty, prevrandao = Some(mix_hash)) with `blob_excess_gas_and_price = expected_blob_excess_gas_and_price(parent_header, spec_id)`.
4. Reconstruct an `OpBlockExecutionCtx { parent_hash, parent_beacon_block_root, extra_data }` from the same header.
5. Return `PartialExecution { tx_hashes, results, expected_state, block_env, op_block_ctx }`.

`parent_header` is threaded through the outer loop so each block's context correctly references its parent; the first block uses the safe-head header, subsequent blocks use the previous block's sealed header.

This approach emits exactly ONE `PartialExecution` covering the entire block. Tests that split one block into multiple partials build sub-partials by calling `partial.split(partials_per_block)` on the captured full-block `PartialExecution` (which threads the running `expected_state` snapshot through each tx boundary via `apply_result_to_expected_state`). The free function `split_collected_partials(partials, partials_per_block)` is provided in `client/core.rs` for tests / mid-block splitting at the prover layer.

#### Scenario: one captured block yields one full-block partial by default
- **WHEN** a block with `N` tx-body transactions is executed and its traces recovered
- **THEN** `recover_collected_partials` produces a `Vec<Vec<PartialExecution>>` with one outer entry containing one `PartialExecution` whose `tx_hashes.len() == N` and whose `expected_state` equals the snapshot captured before tx 0

#### Scenario: `block_env` mirrors the sealed header's fields
- **WHEN** `build_single_partial_for_block` runs against a sealed block's header
- **THEN** the returned `block_env` fields match the header exactly (and `blob_excess_gas_and_price` is derived from `parent_header` + `spec_id` via the shared helper that mirrors kona's `prepare_block_env`)

#### Scenario: `PartialExecution::split` preserves block_env / op_block_ctx and threads expected_state
- **WHEN** `partial.split(partials_per_block)` is called on a block-wide `PartialExecution` (with `partials_per_block > 0`)
- **THEN** every produced sub-partial carries a clone of the original `block_env` and `op_block_ctx`, and each sub-partial's `expected_state` is the running snapshot at its starting tx position (advanced from the parent partial's snapshot via `apply_result_to_expected_state` for each preceding tx)

### Requirement: `PartialResultAndState` is a sorted-Vec mirror of revm's `ResultAndState`

The system SHALL define `PartialResultAndState` in `kailua-kona` at `crates/kona/src/evm/partial.rs` with two fields:

- `result: ExecutionResult<OpHaltReason>` — the per-tx execution result.
- `state: Vec<PartialStateEntry>` — sorted by `address`. Each `PartialStateEntry { address, account: PartialAccount }` with `PartialAccount { info, original_info, transaction_id, status, storage: Vec<PartialStorageEntry> }` (storage sorted by `slot`).

`From<ResultAndState<OpHaltReason>> for PartialResultAndState` SHALL sort the state on construction. `From<PartialResultAndState> for ResultAndState<OpHaltReason>` SHALL rebuild the `HashMap` form.

The struct (and its `PartialAccount` / `PartialStateEntry` / `PartialStorageEntry` building blocks) SHALL derive rkyv `Archive` / `Serialize` / `Deserialize` so that `PartialExecution` can be encoded directly without an outer mirror type.

#### Scenario: state is sorted by address on construction
- **WHEN** `PartialResultAndState::from(ras)` is called for any `ResultAndState`
- **THEN** the returned `state` Vec satisfies `state.windows(2).all(|w| w[0].address < w[1].address)`

#### Scenario: per-account storage is sorted by slot on construction
- **WHEN** `PartialAccount::from(account)` is called for any `Account`
- **THEN** the returned `storage` Vec satisfies `storage.windows(2).all(|w| w[0].slot < w[1].slot)`

#### Scenario: rkyv round-trip preserves sort invariants and (addr, slot, original, present) set equality
- **WHEN** a `PartialResultAndState` is rkyv-serialized and deserialized
- **THEN** the round-tripped value preserves both sort invariants and round-trips back to the original revm `ResultAndState` with set-equality on `(address, slot, original_value, present_value)` tuples

### Requirement: `ExpectedState` covers a fixed, spec-bounded slot set on `L1_BLOCK_CONTRACT`

The system SHALL define in `kailua-kona` at `crates/kona/src/evm/expected.rs`:

- `ExpectedStorageEntry { slot: U256, value: U256 }` — sorted by slot.
- `ExpectedAccount { exists: bool, info: AccountInfo, storage: Vec<ExpectedStorageEntry> }`.
- `ExpectedStateEntry { address: Address, account: ExpectedAccount }` — sorted by address.
- `EXPECTED_STATE_ADDRESSES: [Address; 1] = [L1_BLOCK_CONTRACT]`.
- `EXPECTED_STORAGE_SLOTS: [U256; 6] = [L1_BASE_FEE_SLOT, ECOTONE_L1_FEE_SCALARS_SLOT, L1_OVERHEAD_SLOT, L1_SCALAR_SLOT, ECOTONE_L1_BLOB_BASE_FEE_SLOT, OPERATOR_FEE_SCALARS_SLOT]` (constants pulled from `alloy_evm::op_revm::constants::*`).
- `capture_required_expected_state<DB: Database>(db: &mut DB) -> Vec<ExpectedStateEntry>` — reads the cartesian product `EXPECTED_STATE_ADDRESSES × EXPECTED_STORAGE_SLOTS` from the live DB and returns the canonicalized snapshot.
- `apply_result_to_expected_state(&mut snapshot, &result)` — advances the snapshot by one tx's state diff, only updating addresses already in `EXPECTED_STATE_ADDRESSES` and slots already in the snapshot. NEVER inserts new accounts or new slots. Then re-sorts.
- `canonicalize_expected_state(snapshot)` — sorts entries by address and per-account storage by slot.

The fixed slot set covers OP fee calculation reads that happen outside the per-tx state diff. The `apply_result_to_expected_state` invariant (no new slots) bounds the snapshot to what `capture_required_expected_state` re-derives, ensuring the host's pre-hash input matches the guest's re-derive.

#### Scenario: capture reads exactly the spec-bounded slot set
- **WHEN** `capture_required_expected_state(db)` is called
- **THEN** it issues exactly `EXPECTED_STATE_ADDRESSES.len()` `Database::basic` calls and `EXPECTED_STATE_ADDRESSES.len() × EXPECTED_STORAGE_SLOTS.len()` `Database::storage` calls — no more, no less

#### Scenario: `apply_result_to_expected_state` does not add new slots
- **WHEN** a tx result writes to a slot NOT already in `expected_state`
- **THEN** `expected_state` is left unchanged in slot count — the splitter's per-tx snapshot stays bounded

#### Scenario: round-trip equality between host capture and guest re-derive
- **WHEN** the host calls `capture_required_expected_state(seed_db)` to produce `original`, builds a witness with `cache = cache_results(results, original.clone())`, and the guest re-derives via `capture_required_expected_state(state_with_cached_prestate(witness.cache))` to produce `derived`
- **THEN** `hash_expected_state(&original) == hash_expected_state(&derived)`

### Requirement: `PartialExecutionWitness` carries the inputs for a single partial proof

The system SHALL define a `PartialExecutionWitness` struct in `kailua-kona` at `crates/kona/src/evm/witness.rs` with four fields:

- `transactions: Vec<Vec<u8>>` — the ordered, RLP-encoded EIP-2718 transactions to re-execute.
- `cache: CacheState` — the pre-state the guest seeds into `State::builder().with_cached_prestate(cache)`. Serialized via `CacheStateRkyv`. Implicitly carries the partial's starting `ExpectedState` snapshot (folded in by `cache_results`).
- `block_env: BlockEnv` — the block environment (serialized via `BlockEnvRkyv`).
- `op_block_ctx: OpBlockExecutionCtx` — the OP block context (serialized via `OpBlockExecutionCtxRkyv`).

A `PartialExecutionWitness::new(partial: PartialExecution, transactions: Vec<Vec<u8>>) -> Self` constructor SHALL:

1. Destructure `partial` into `(tx_hashes, results, expected_state, block_env, op_block_ctx)`.
2. Build `cache` via `cache_results(results, expected_state)`.
3. Carry `block_env` and `op_block_ctx` through unchanged.

A `PartialExecutionWitness::from_preflight(partial: PartialExecution, execution: &Execution) -> Self` constructor SHALL look up transaction bytes via `execution.get_transactions(&partial.tx_hashes)` (in `crates/kona/src/executor.rs`) and forward to `new`.

`cache_results(results, expected_state) -> CacheState` SHALL:

1. First fold `expected_state` entries into `cache.accounts`: each `exists: true` entry inserts a `CacheAccount::new_loaded(info, storage_map)` and lifts any inline `info.code` into `cache.contracts` keyed by `code_hash`; each `exists: false` entry inserts a `CacheAccount::new_loaded_not_existing()`.
2. Then fold `partial.results` through to insert each address's `original_info` + first-load storage. Bytecode in `info.code` is lifted into `cache.contracts` keyed by `code_hash`. Existing entries are NOT overwritten (`or_insert` semantics) — first-load wins.

The witness does NOT need to ship an `EvmAccumulatorState`, a `tx_count`, a block number, a chunk index, per-tx metadata (da_footprint / blob_gas deltas / block-hash reads), an explicit `expected_state` field, or positional block-level commitments (`agreed_l2_output_root`, `config_hash`, etc.) — those either live in the `BootInfo` (which is always shipped), are folded into `cache`, or are reconstructable at proof time from the four fields above.

#### Scenario: witness ingestion produces a valid partial prestate
- **WHEN** a `PartialExecution` is converted via `from_preflight`
- **THEN** the resulting witness contains (a) transactions resolvable from `execution.attributes.transactions`, (b) a `CacheState` covering every address / slot / contract that any tx in the partial first-loaded AND every L1Block slot in the partial's `expected_state`, (c) the original `block_env` / `op_block_ctx`

#### Scenario: `cache_results` deduplicates first-load entries
- **WHEN** multiple transactions in the same partial touch the same account or storage slot
- **THEN** only the first-load `original_info` / `original_value` is inserted into the resulting `CacheState` (later overwrites are ignored)

#### Scenario: `cache_results` lifts inline bytecode into the contracts map
- **WHEN** a transaction's account `original_info.code` is `Some(bytecode)` (or an `expected_state` entry's `info.code` is `Some(bytecode)`)
- **THEN** `cache_results` inserts `(code_hash, bytecode)` into `cache.contracts` exactly once per code hash

#### Scenario: rkyv round-trip preserves all fields
- **WHEN** a populated `PartialExecutionWitness` is rkyv-serialized and deserialized
- **THEN** `transactions`, `block_env`, `op_block_ctx`, and the `cache.accounts` / `cache.contracts` / `cache.has_state_clear` fields round-trip byte-for-byte

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

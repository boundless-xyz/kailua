## ADDED Requirements

### Requirement: `run_witgen_client` threads partial data end-to-end

`run_witgen_client` in `kailua-prover` at `crates/prover/src/client/witgen.rs` SHALL accept a `partial_executions: Vec<Vec<PartialExecution>>` parameter, forward it unchanged to `run_core_client`, and persist it into the emitted `Witness` so downstream guest proofs re-receive the exact positional partials the host already captured. The emitted `Witness.pe_witness` SHALL be `None` for the witness-generation path (witness-generation never produces partial proofs itself — it produces the aggregation witness).

All callers (`kailua-hana`, `kailua-hokulea`, `kailua-prover::proving`) SHALL pass `Vec::new()` until the prover dispatch layer (see requirement below) supplies a non-empty vec.

#### Scenario: empty partials is the no-op default
- **WHEN** `run_witgen_client(.., partial_executions = Vec::new(), ..)` is called
- **THEN** the emitted `Witness` has `pe_witness == None` and `partial_executions == Vec::new()`, and downstream behavior is byte-identical to the pre-partials code path

#### Scenario: non-empty partials round-trip into the Witness
- **WHEN** `run_witgen_client` is called with a non-empty `partial_executions`
- **THEN** the emitted `Witness.partial_executions` contains the same `Vec<Vec<PartialExecution>>`, ready to be served by `CachedEvmFactory` on the downstream proof

### Requirement: `max_txs_per_chunk` configuration parameter *(future work)*

The prover SHALL accept a positive `max_txs_per_chunk: usize` argument (task 9.1, not yet implemented) with a default of `usize::MAX` (chunking disabled). `max_txs_per_chunk = 0` MUST be rejected before proving begins. When set to a value less than a block's transaction count, the prover SHALL decompose that block into multiple `PartialExecution` entries dispatched in parallel.

#### Scenario: default disables decomposition
- **WHEN** `max_txs_per_chunk` is not specified
- **THEN** every block is proven monolithically (existing behavior)

#### Scenario: explicit value triggers decomposition
- **WHEN** `max_txs_per_chunk = 4` and a block has 10 transactions
- **THEN** the prover builds 3 partials ([0..3], [4..7], [8..9]) and dispatches them in parallel

#### Scenario: value ≥ block tx count stays monolithic
- **WHEN** `max_txs_per_chunk = 100` and a block has 5 transactions
- **THEN** the prover keeps the existing monolithic proving path and does not decompose

#### Scenario: zero is rejected
- **WHEN** `max_txs_per_chunk = 0`
- **THEN** the prover returns a configuration error before any proof is dispatched

### Requirement: Prover dispatches partial proofs in parallel *(future work)*

When decomposition is active for a block the prover SHALL (task 9.2, not yet implemented):

1. Pre-execute the block through the main witgen path (which already uses `CachedEvmFactory` with a trace collector) to capture per-transaction `(tx_hash, ResultAndState)` traces.
2. Slice the block's single captured `PartialExecution` into sub-partials bounded by `max_txs_per_chunk`, cloning the shared `block_env` / `op_block_ctx`.
3. For each sub-partial build a `PartialExecutionWitness::from_preflight(partial.clone(), execution)` and dispatch a partial proof job (l1_head = `0xFF..FF`, `Witness.pe_witness = Some(witness)`).
4. Collect partial proof receipts and append them to `stitched_proofs`.

#### Scenario: parallel dispatch respects concurrency cap
- **WHEN** a block is split into 4 partials and `num_concurrent_proofs = 2`
- **THEN** at most 2 partial proof jobs run in parallel at any moment

#### Scenario: partial proof failure aborts the block
- **WHEN** any partial proof job fails
- **THEN** the prover reports the error and does not assemble a partial-aggregation witness for that block

### Requirement: Prover assembles aggregation witness from partial proofs *(future work)*

After all partial proof receipts are collected the prover SHALL assemble (task 9.3, not yet implemented) the block aggregation witness:

- The block's positional `Vec<PartialExecution>` (populated with each partial's `tx_hashes` / `results` / `block_env` / `op_block_ctx`) — inserted into `Witness.partial_executions` at the matching outer index.
- The partial proof receipts appended to `Witness.stitched_proofs`.
- No independent state preimages or cache structures beyond what the existing monolithic witgen already emits — `CachedEvm` authenticates prestate directly against the aggregation `TrieDB`.

#### Scenario: aggregation witness is complete
- **WHEN** the prover assembles the aggregation witness
- **THEN** the guest can (a) verify every partial proof receipt via `stitch_partial_executions`, (b) replay partials through `CachedEvm` against the real `TrieDB`, and (c) produce the final block's `BlockBuildingOutcome` without any additional host input

#### Scenario: partial receipts flow through `stitched_proofs`
- **WHEN** partial receipts are assembled into the witness
- **THEN** they are appended to the `stitched_proofs` vector alongside any block-level stitching receipts

### Requirement: Partial decomposition composes with block-level splitting

Transaction-level partial decomposition SHALL compose with the existing block-level splitting (task 10.3, not yet implemented) (`max_block_executions`, witness size limits). When both are active the block-level splitter divides a block range into execution-only proof jobs, and within each job the tx-level decomposition further splits individual blocks' tx bodies.

#### Scenario: block splitting + partial decomposition
- **WHEN** `max_block_executions = 2` and `max_txs_per_chunk = 5` with blocks of 20 transactions each
- **THEN** blocks are grouped into execution-only jobs of 2 blocks, and within each block tx-level decomposition produces 4 partials of 5 transactions

#### Scenario: block splitting without tx-level decomposition
- **WHEN** `max_block_executions = 2` and `max_txs_per_chunk = usize::MAX`
- **THEN** blocks are grouped into execution-only jobs of 2 blocks and proven monolithically (existing behavior)

### Requirement: `--num-block-partials` configuration parameter

The prover SHALL accept a non-negative `num_block_partials: usize` argument on `ProvingArgs` in `crates/prover/src/args.rs`, exposed as the `--num-block-partials` CLI flag (alias env var). The default SHALL be `0` (chunking disabled — every block is proven monolithically). When set to a positive value `N`, the prover SHALL decompose each non-empty block's full-block `PartialExecution` into up to `N` sub-partials via `PartialExecution::split(N)`.

#### Scenario: default disables decomposition
- **WHEN** `--num-block-partials` is not specified
- **THEN** the prover defaults to `0`, every block is proven monolithically, and the prover dispatches no partial proof jobs (existing behavior)

#### Scenario: explicit value triggers decomposition
- **WHEN** `--num-block-partials 4` and a block has 10 transactions
- **THEN** the prover builds up to 4 partials (`tx_count.div_ceil(4).max(1)` = 3 txs each: 3, 3, 3, 1) and dispatches them in parallel

#### Scenario: zero is the disabled default
- **WHEN** `num_block_partials == 0`
- **THEN** `PartialExecution::split(0)` returns an empty `Vec`, no partial proof jobs are dispatched, and the aggregation pass runs against `partial_executions = Vec::new()` — preserving the monolithic byte-for-byte path

### Requirement: `concurrent_preflight` returns captured per-block partials

`concurrent_preflight` in `crates/prover/src/preflight.rs` SHALL return a `(bool, Vec<Execution>, Vec<Vec<PartialExecution>>)` triple. The third element captures one full-block `PartialExecution` per non-empty block in execution order, populated from each preflight worker's `recover_collected_partials` invocation. Each preflight worker SHALL run with a `partials_collector` attached so the trace collector populates correctly.

#### Scenario: every preflight-captured block yields one full-block partial
- **WHEN** preflight produces `N` block executions
- **THEN** the returned `Vec<Vec<PartialExecution>>` has `N` outer entries, each containing one full-block `PartialExecution` with the block's complete tx body

### Requirement: Prover dispatches partial proofs in parallel

When `num_block_partials > 0`, `prove.rs` SHALL after preflight:

1. Iterate `block_executions.zip(partial_executions)` lock-step.
2. Skip blocks with empty `attributes.transactions` (system-tx-only blocks).
3. For each remaining block, pop the block's full-block partial; sanity-check that `exec.artifacts.header.parent_hash == partial.op_block_ctx.parent_hash`.
4. Slice the partial via `partial.split(args.proving.num_block_partials)`.
5. For each sliced sub-partial:
    - Insert into `partial_proof_cache: PartialsCache = BTreeMap<u64, Vec<PartialExecution>>` keyed by `parent_block_number = exec.artifacts.header.number - 1`.
    - Spawn an async task that calls `compute_oneshot_task(None, job_args, ..., precondition: Precondition::default(), proposal_data_hash: B256::ZERO, stitched_executions: vec![vec![execution]], derivation_cache: None, ..., stitched_preconditions: vec![], stitched_boot_info: vec![], partial_executions: vec![vec![partial]], stitched_proofs: vec![], prove_snark: false, force_attempt: true, seek_proof: true, task_sender)` where `job_args.kona = SingleChainHost { l1_head: 0xFF..FF, agreed_l2_head_hash: parent_hash, agreed_l2_output_root: parent_hash, claimed_l2_output_root: parent_hash, claimed_l2_block_number: parent_block_number, ..args.kona }`.
6. Await all dispatched partial-proof tasks; log any failures.
7. Wrap `partial_proof_cache` in `Arc` and pass to the aggregation pass.

The partial proof job in turn invokes `client/native::run_native_client`, which detects `args.kona.l1_head == 0xFF..FF`, pops one partial from the supplied `partial_executions`, pops the matching `Execution`, and constructs a `PartialExecutionWitness::from_preflight(partial, &exec)` to pass into `run_proving_client`.

#### Scenario: parallel dispatch respects concurrency cap
- **WHEN** a block is split into 4 partials and `num_concurrent_proofs = 2`
- **THEN** at most 2 partial proof jobs run in parallel at any moment (gated by the prover task pool of `num_concurrent_proofs` `handle_oneshot_tasks` workers)

#### Scenario: partial proof failure is logged
- **WHEN** any partial proof job fails
- **THEN** the prover logs the error and continues — failure of a single partial does not abort the whole prove run

#### Scenario: empty block is skipped
- **WHEN** a block has no tx-body transactions (deposit-only or empty)
- **THEN** no partial proof job is dispatched for that block (no `0xFF..FF` boot is created); the `partial_proof_cache` does not include the block

### Requirement: `PartialsCache` keys partials by parent block number

The prover SHALL define `PartialsCache = BTreeMap<u64, Vec<PartialExecution>>` in `crates/prover/src/client/native.rs` keyed by `parent_block_number`. The cache SHALL be threaded as `Option<Arc<PartialsCache>>` through `CachedTask`, `compute_oneshot_task`, `compute_cached_proof`, `run_native_client`, and `run_proving_client`.

The aggregation pass receives the cache and re-supplies per-block partials when building each block's range proof, populating `Witness.partial_executions` so the aggregation guest serves cached results via `CachedEvmFactory`.

#### Scenario: BTreeMap key is parent block number
- **WHEN** a partial for block `N` is inserted into the cache
- **THEN** the entry key is `N - 1` (the parent block number, matching the partial's `claimed_l2_block_number`)

#### Scenario: cache is `None` for partial-proof tasks
- **WHEN** a partial proof job is dispatched (l1_head = 0xFF..FF)
- **THEN** `partials_cache: None` is passed to `compute_oneshot_task` — partial-proof jobs do not need (and do not consult) the cache

### Requirement: `compute_cached_proof` updates the proof journal precondition for partial mode

In `crates/prover/src/tasks.rs`, `compute_cached_proof` SHALL after `stitch_boot_info` returns, when `boot.l1_head == B256::repeat_byte(0xFF)`:

1. Read the first `partial` from the supplied `partial_executions`.
2. Update `updated_precondition = updated_precondition.partial(partial.precondition_hash())`.
3. Recompute `proof_journal.precondition_hash = B256::new(updated_precondition.digest().into())`.

This ensures the on-disk proof file name (and the cache lookup key) matches the actual partial's precondition. If the supplied `partial_executions` is empty in partial mode, the prover logs an error and proceeds with whatever precondition was previously computed (caller error).

#### Scenario: partial-mode precondition is folded into the journal
- **WHEN** a partial proof task is computed with `boot.l1_head == 0xFF..FF` and a non-empty `partial_executions`
- **THEN** `proof_journal.precondition_hash` equals `Digest::from_bytes(partial.precondition_hash().0)` (i.e., `Precondition::default().partial(partial.precondition_hash()).digest()`)

#### Scenario: missing partial in partial-mode is logged
- **WHEN** `compute_cached_proof` is called with `boot.l1_head == 0xFF..FF` and an empty `partial_executions`
- **THEN** an error is logged ("Failed to find partial execution for precondition") and the journal precondition retains its prior value

### Requirement: `run_witgen_client` threads partial data end-to-end

`run_witgen_client` in `kailua-prover` at `crates/prover/src/client/witgen.rs` SHALL accept `pe_witness: Option<PartialExecutionWitness>` and `partial_executions: Vec<Vec<PartialExecution>>` parameters, forward both unchanged to `run_core_client`, capture the trace collector's output via `recover_collected_partials` after the core run (when `trace_partials` is enabled), and persist `pe_witness` + the captured / supplied `partial_executions` into the emitted `Witness`.

#### Scenario: empty partials is the no-op default
- **WHEN** `run_witgen_client(.., pe_witness = None, partial_executions = Vec::new(), trace_partials = false, ..)` is called
- **THEN** the emitted `Witness` has `pe_witness == None` and `partial_executions == Vec::new()`, and downstream behavior is byte-identical to the pre-partials code path

#### Scenario: non-empty partials round-trip into the Witness
- **WHEN** `run_witgen_client` is called with a non-empty `partial_executions` (or `trace_partials = true` and the core run captures partials)
- **THEN** the emitted `Witness.partial_executions` contains the same `Vec<Vec<PartialExecution>>`, ready to be served by `CachedEvmFactory` on the downstream proof

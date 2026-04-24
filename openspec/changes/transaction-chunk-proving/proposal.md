## Why

Full blocks containing many expensive transactions create a proving-latency bottleneck. Today the smallest provable unit is an entire block — every transaction is proven monolithically inside a single zkVM execution. When a block is packed with high-cost transactions (large contract deployments, complex DeFi interactions), proving time is dominated by one long-running job with no opportunity for parallelism.

By decomposing a block's ordered transaction body into independently provable **partial executions**, we enable parallel proving across multiple machines. Each partial carries the transactions it proved plus the full per-transaction `ResultAndState` trace it produced. At aggregation time those cached results flow back through the standard block executor, with a single per-tx prestate-authentication check inside the EVM layer ensuring the cached results match the real state. The output is indistinguishable from today's monolithic block proof, so block stitching, derivation, fault proofs, and on-chain verification require no changes.

## What Changes

- **Unified `CachedEvm` / `CachedEvmFactory`.** A single `Evm`-trait wrapper replaces four earlier types (tracing evm, chunking evm, their factories). Depending on how it is constructed it serves as a transparent pass-through, a trace collector (capturing per-tx `(tx_hash, ResultAndState)` pairs during host pre-execution), or a cached-serve EVM that returns witness-supplied results AFTER authenticating their `original_info` / `original_value` against the live `TrieDB`. Zero vendored code changes.
- **`PartialExecution` struct.** A proven transaction subsequence carries only four fields: `tx_hashes`, `results` (full per-tx `ResultAndState<OpHaltReason>`), `block_env`, `op_block_ctx`. There are no pre/post memory-DB hashes, no EVM-accumulator state, no `tx_count`, no positional block metadata — everything else is derived at verification time.
- **`PartialExecutionWitness` struct.** The single-partial proof witness carries `transactions` + a pre-state `CacheState` (seeded into `State::builder().with_cached_prestate(...)`) + `block_env` + `op_block_ctx`. No `PanicDB`: the partial guest runs against a real `TrieDB` wrapped in `State` with the `CacheState` as its lazy prestate layer.
- **Precondition extension.** A new `partial_executions: B256` field on `Precondition` (third exclusive mode alongside execution-only and derivation/proposal), with digest `compute_pe_trace(results_hash, block_ctx_hash)` — a two-input `SHA256(results_hash || block_ctx_hash)`. `results_hash` folds each account's `original_info` into the canonical encoding, which is what makes the per-tx prestate authentication cryptographically binding.
- **Partial proving mode.** A new `PARTIAL EXECUTION` branch in `run_core_client` activated by `boot.l1_head == 0xFF..FF`. It seeds the carried `CacheState` into `State`, runs every tx through the standard `OpBlockExecutor` with `CachedEvmFactory` capturing traces, and emits a `ProofJournal` with `l1_head = 0xFF..FF` and `agreed_l2_output_root == claimed_l2_output_root`.
- **Unified aggregation path.** Both the `EXECUTION ONLY` and `DERIVATION & EXECUTION` branches of `run_core_client` build their executor with `CachedEvmFactory::new_with_traces(partial_executions, collector)`. With an empty partials vec this is byte-identical to `OpEvmFactory::default()`; with populated partials it serves cached results with inline prestate authentication. No separate "aggregation mode".
- **Stitching extension.** Two new free functions: `precompute_pe_boots` (run BEFORE `run_core_client` consumes its partials) produces `(precondition_hash, StitchedBootInfo)` pairs; `stitch_partial_executions` reconstructs the partial `ProofJournal` and verifies it against `proven_fpvm_journals`. No `verify_block_chunks` helper — block-context authentication lives inline in `CachedEvm::transact_raw`.
- **Witness threading.** `Witness` gains `pe_witness: Option<PartialExecutionWitness>` and `partial_executions: Vec<Vec<PartialExecution>>`. Both thread through `run_stateless_client` → `run_stitching_client` → `run_core_client` and through `StitchingClient` implementations in hana / hokulea / prover witgen.
- **Prover-side dispatch** *(task 9, not yet shipped).* A `max_txs_per_chunk` configuration parameter will let the prover slice a block's captured full-block `PartialExecution` into sub-partials, dispatch partial proof jobs in parallel, collect receipts, and assemble the aggregation witness.

## Capabilities

### New Capabilities

- `partial-precondition` — Extension of `Precondition` with a `partial_executions` field and the two-input `compute_pe_trace` digest.
- `partial-proving` — The `PARTIAL EXECUTION` branch in `run_core_client` that executes a single `PartialExecutionWitness` against a `State<TrieDB>` seeded with the witness `CacheState`, captures per-tx results, and emits a partial `ProofJournal`.
- `partial-aggregation` — `PartialExecution` struct, `CachedEvm`'s per-tx prestate authentication, and the `precompute_pe_boots` + `stitch_partial_executions` pipeline that verifies partial receipts alongside execution-only proofs.
- `partial-witness-construction` — Host-side assembly of `PartialExecution` entries from the `CachedEvmFactory` trace collector (`recover_collected_partials` + `build_single_partial_for_block`) and of `PartialExecutionWitness` from a `PartialExecution` + `Execution` (`PartialExecutionWitness::from_preflight`).
- `cached-evm-factory` — The unified `CachedEvm` / `CachedEvmFactory` EVM wrapper replacing the four-type tracing/chunking split, plus the `EvmFactory`-generic refactor of `CachedExecutor`.
- `prover-partial-dispatch` — The `run_witgen_client` → `Witness` threading is shipped; `max_txs_per_chunk` configuration, parallel dispatch, and block-level composition remain future work (tasks 9.x / 10.3).

### Modified Capabilities

_(none — partial execution is transparent to existing block stitching, derivation, and fault proof systems)_

## Impact

- **`crates/kona/src/`** — New `evm.rs` (`CachedEvm`, `CachedEvmFactory`, `PartialExecution`, `PartialExecutionWitness`, `TransactionResultCollector`). New `precondition/evm.rs` (`compute_pe_trace`, `hash_results`, `hash_block_ctx`, their flatten helpers). Modified `precondition/mod.rs` (new field + builder + digest branch). Modified `witness.rs` (new `pe_witness` / `partial_executions` fields). Modified `executor.rs` (`CachedExecutor` generic over any `EvmFactory`; `build_single_partial_for_block`; `expected_blob_excess_gas_and_price`). Modified `client/core.rs` (PARTIAL EXECUTION branch, `validate_cache`, `recover_collected_partials`, unified `CachedEvmFactory` routing). Modified `client/stitching.rs` (trait signature, `precompute_pe_boots`, `stitch_partial_executions`). Extended `rkyv/evm.rs` with `CacheStateRkyv`, `BlockEnvRkyv`, `OpBlockExecutionCtxRkyv`, `ResultAndStateRkyv`, `AccountRkyv`, `EvmStateRkyv`, `ExecutionResultRkyv`, and the `halt_reason_rkyv` helpers.
- **`crates/prover/src/`** — Modified `client/witgen.rs` (`run_witgen_client` threads `partial_executions`). No `chunk` module (witness assembly lives in `kailua-kona`). `args.rs`, `tasks.rs`, `proving.rs` still need `max_txs_per_chunk` integration (task 9).
- **`build/risczero/*/src/main.rs`** — Entry points thread `pe_witness` and `partial_executions` through to `run_stateless_client`.
- **Vendored code (`optimism/rust/`)** — No changes.
- **External dependencies** — No new dependencies. Uses existing revm primitives.
- **Backward compatibility** — Fully backward compatible. A witness with `pe_witness = None` and empty `partial_executions` exercises the same code path as before (with `CachedEvmFactory` degrading to an `OpEvmFactory` pass-through). Existing proofs, stitching receipts, and fault-proof games are unaffected.

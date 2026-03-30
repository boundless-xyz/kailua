## Why

Full blocks containing many expensive transactions create a proving latency bottleneck. Today, the smallest provable unit is an entire block — all transactions are proven monolithically inside a single zkVM execution. When a block is full of high-cost transactions (large contracts, complex DeFi interactions), the proving time is dominated by the single longest proof job with no opportunity for parallelism.

By decomposing block execution into independently provable transaction chunks, we enable parallel proving across multiple machines. The chunks are aggregated into a block-level proof that is identical to today's monolithic output, making the change transparent to the existing stitching, derivation, and fault proof systems.

## What Changes

- **TracingEvmFactory**: A new `EvmFactory` implementation that wraps `OpEvmFactory` and captures per-transaction `EvmState` from `transact_raw()` during host-side pre-execution. This provides the state access traces needed to build chunk witnesses. Zero vendored code changes.
- **Precondition struct extension**: A new `chunk_trace` field on the `Precondition` struct, with corresponding `Digestible` implementation for chunk-only preconditions.
- **Chunk proving mode**: A guest execution mode that operates on a flat in-memory state (`CacheDB<PanicDB>`) rather than a full state trie. Executes only a configured subset of the block's ordered transaction body against the post-prelude state and emits a standard `ProofJournal` with `l1_head = 0xFF..FF` as the chunk sentinel.
- **Block aggregation mode**: A CachedExecutor path that applies block-level prelude once before chunk verification, verifies chunk proof receipts, chains memory DB and EVM state hashes for continuity, applies block-level epilogue once after the final chunk, and produces a standard `BlockBuildingOutcome` — identical to monolithic execution output.
- **Chunk witness construction**: Host-side logic that uses per-transaction traces to compute chunk-start flat state witnesses from boundary snapshots, carrying forward full account metadata and storage dependencies between chunks.
- **Prover integration**: New `max_txs_per_chunk` configuration parameter. When set, the prover activates chunk decomposition for blocks, dispatches chunk proofs in parallel, collects receipts, and assembles the aggregation witness.
- **Memory DB hashing**: Deterministic SHA256 hashing of revm's `Cache` structure (accounts, contracts, block_hashes) for pre/post chunk state commitments.
- **EVM state hashing**: Deterministic SHA256 hashing of cross-chunk accumulators (cumulative gas, DA footprint, blob gas, logs bloom, receipts) with pre/post continuity chaining.

## Capabilities

### New Capabilities

- `tracing-evm-factory`: TracingOpEvm and TracingEvmFactory that wrap OpEvm/OpEvmFactory to capture per-transaction EvmState during host-side execution, with zero vendored code changes.
- `chunk-precondition`: Extension of the Precondition struct with a `chunk_trace` field and its digest computation, covering tx_hash, pre/post memory DB hashes, and pre/post EVM state hashes.
- `memory-db-hashing`: Deterministic canonical hashing of revm's Cache structure (accounts, storage, contracts, block_hashes) and EVM state accumulators for chunk state commitments.
- `chunk-proving`: Guest execution mode for proving a transaction chunk against a flat CacheDB<PanicDB>, emitting a standard ProofJournal with chunk sentinel l1_head.
- `chunk-aggregation`: Block-level aggregation that verifies chunk receipts, chains pre/post hashes for continuity, computes the final trie root from the composed state, and produces a standard BlockBuildingOutcome.
- `chunk-witness-construction`: Host-side algorithm that uses per-transaction EvmState traces to build minimal flat state witnesses per chunk, handling cross-chunk state dependencies.
- `prover-chunk-dispatch`: Prover-side orchestration that splits blocks into transaction chunks, dispatches chunk proofs in parallel, collects receipts, and assembles the aggregation witness.

### Modified Capabilities

_(none — chunking is transparent to existing block stitching, derivation, and fault proof systems)_

## Impact

- **`crates/kona/src/`**: New files for tracing EVM factory, chunk proving, chunk aggregation, memory DB hashing. Modified `executor.rs` (CachedExecutor generics), `precondition/mod.rs` (new field), `witness.rs` (new witness fields), `client/core.rs` (thread chunk data to executor).
- **`crates/prover/src/`**: Modified `tasks.rs` (chunk dispatch logic), `args.rs` (new `max_txs_per_chunk` parameter), `prove.rs` (chunk proof collection).
- **`build/risczero/*/src/main.rs`**: Guest entry points gain chunk execution mode routing.
- **Vendored code (`optimism/rust/`)**: No changes.
- **External dependencies**: No new dependencies. Uses existing revm `CacheDB`, `Cache`, `PanicDB` (or equivalent empty DB that panics on reads).
- **Backward compatibility**: Fully backward compatible. `max_txs_per_chunk = usize::MAX` (default) disables chunking entirely. Existing proofs, stitching, and fault proof games are unaffected.

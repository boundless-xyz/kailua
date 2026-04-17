## Context

Kailua proves OP-Stack rollup state transitions using RISC Zero's zkVM. The current architecture proves blocks monolithically: all transactions in a block execute inside a single zkVM guest execution, producing a `ProofJournal` that attests to the state transition. Blocks are composed via a stitching system that chains `ProofJournal` entries and verifies them recursively via `env::verify()`.

The execution pipeline is:
1. **Host**: Pre-executes blocks, assembles witnesses with preimage data.
2. **Guest**: Executes blocks statelessly via `StatelessL2Builder::build_block()`, using `CachedExecutor` to leverage pre-computed results.
3. **Stitching**: `stitch_executions()` and `stitch_boot_info()` compose multiple block proofs into range proofs.

The `Precondition` struct tracks proof assumptions with four fields: `proposal_blobs`, `execution_trace`, `derivation_cache`, `derivation_trace`. The `Digestible` implementation dispatches between execution-only (when `execution_trace != 0`) and combined derivation/proposal modes.

The guest programs (`kailua-fpvm-kona`, `kailua-fpvm-hana`, `kailua-fpvm-hokulea`) share the same execution flow via the `StitchingClient` trait and always emit a 220-byte packed `ProofJournal`.

## Goals / Non-Goals

**Goals:**
- Reduce proving latency for blocks with expensive transactions by enabling parallel chunk proving.
- Integrate cleanly into the existing `ProofJournal`, `Precondition`, and stitching system — chunk proofs emit standard `ProofJournal`, block aggregation produces standard `BlockBuildingOutcome`.
- Zero modifications to vendored code in `optimism/rust/`.
- Configurable granularity via `max_txs_per_chunk`.
- Full backward compatibility — disabled by default.

**Non-Goals:**
- Automatic chunk sizing based on transaction cost estimation.
- State-access conflict analysis for optimal parallelism (transactions are chunked sequentially by index).
- Cross-block transaction chunking (chunks are always within a single block).
- Modifying the on-chain verification contracts or fault proof game.

## Decisions

### Decision 1: Custom EvmFactory for state tracing (not vendored modifications)

**Choice**: Create `TracingEvmFactory` + `TracingOpEvm` that wrap `OpEvmFactory`/`OpEvm`, intercepting `transact_raw()` to clone per-transaction `EvmState` for the ordered block transaction body only.

**Alternatives considered**:
- *Modify `build_block()` to accept a state hook* (~10 lines in kona fork): Simpler but modifies vendored code.
- *Inspector-based tracing via `create_evm_with_inspector`*: Operates at instruction level (SLOAD/SSTORE), more overhead, wrong granularity.
- *Two-pass execution (execute then re-execute with tracing)*: Wasteful — `execute_block` builds a new block, not a replay.
- *Database wrapper intercepting `commit()`*: Type-level complications with the Inspector generic parameter.

**Rationale**: The `EvmFactory` is a generic parameter throughout `KonaExecutor<P, H, Evm>` and `StatelessL2Builder<P, H, Evm>`. Replacing it requires only changing `CachedExecutor::new()` generics — the vendored code's generic plumbing carries our factory through unchanged. The `Evm` trait has 8 methods; only `transact_raw()` needs interception for chunk tracing (a `.clone()` on the result state). `transact_system_call()` remains a transparent delegation because block-level prelude and epilogue are handled outside chunk proving and outside the chunk trace buffer.

### Decision 2: Reuse ProofJournal for chunk proofs (not a new journal type)

**Choice**: Chunk proofs emit standard 220-byte `ProofJournal` with `l1_head = 0xFF..FF` as sentinel.

**Fields for a chunk proof**:
| Field | Value |
|-------|-------|
| `payout_recipient` | Identical to block proof |
| `precondition_hash` | `SHA256(tx_hash \|\| pre_db_hash \|\| post_db_hash \|\| pre_evm_hash \|\| post_evm_hash)` |
| `l1_head` | `0xFF..FF` (chunk sentinel) |
| `agreed_l2_output_root` | Current agreed L2 output root |
| `claimed_l2_output_root` | Same as agreed (chunk doesn't advance L2 state) |
| `claimed_l2_block_number` | Parent block number |
| `config_hash` | Identical to block proof |
| `fpvm_image_id` | Identical to block proof |

**Rationale**: Reusing `ProofJournal` means chunk receipts flow through the existing `load_stitching_journals()` / `verify_stitching_journal()` infrastructure with zero changes. The sentinel values (`l1_head = 0xFF..FF`, `agreed == claimed`) distinguish chunks from block proofs and execution-only proofs (`l1_head = 0x00..00`).

### Decision 3: New `chunk_trace` field on Precondition (not overloading existing fields)

**Choice**: Add `pub chunk_trace: B256` to `Precondition`. The `Digestible` impl treats it as a third exclusive mode (alongside execution-only and derivation/proposal).

**Rationale**: The chunk precondition covers fundamentally different data (tx hash + pre/post state/EVM hashes) than `execution_trace` (which hashes `Execution` structs). Overloading `execution_trace` would conflate two distinct concepts. A dedicated field keeps the semantics clean and the assertion logic in `Digestible::digest()` straightforward.

### Decision 4: Flat CacheDB<PanicDB> for chunk execution (not TrieDB)

**Choice**: Chunk guests execute against revm's `CacheDB` backed by a `PanicDB` that panics on any fallback read.

**Alternatives considered**:
- *TrieDB with minimal preimages*: Would force intermediate trie hashing between chunks, which we explicitly avoid.
- *Custom FlatStateDB implementation*: Unnecessary — revm already provides `CacheDB<ExtDB>` which is exactly a flat in-memory DB with a fallback.

**Rationale**: `CacheDB` is the same structure that `State<TrieDB>` uses internally as its cache layer. Reusing it ensures identical data representation. `PanicDB` as the fallback guarantees completeness — if the host's chunk witness is missing any state, the guest panics rather than silently using wrong data.

### Decision 5: Pre/post hashing for both memory DB and EVM state

**Choice**: The chunk `precondition_hash` commits to five values: `tx_hash`, `pre_db_hash`, `post_db_hash`, `pre_evm_state_hash`, `post_evm_state_hash`.

**Rationale**: Pre/post splitting enables the block aggregation proof to chain both dimensions in lockstep:
- `post_db_hash[i] == pre_db_hash[i+1]` (memory DB continuity)
- `post_evm_hash[i] == pre_evm_hash[i+1]` (EVM accumulator continuity)

The aggregation proof never re-executes transactions — it only verifies hash chains and chunk receipts.

### Decision 6: Memory DB hash covers the execution-relevant flat state and excludes logs

**Choice**: The deterministic SHA256 hash includes accounts (info + storage + account_state), contracts (code_hash set after validating each cached bytecode against its key), and block_hashes. `Cache.logs` is intentionally excluded.

**Rationale**: The chunk chain uses the memory DB hash to commit the execution-relevant flat state that must carry across chunk boundaries. Raw bytecode does not need to be duplicated in the hash encoding if every cached contract loaded from untrusted witness data is checked once to ensure `bytecode.hash_slow() == code_hash`; that one-time ingestion validation authenticates the executed code preimage while keeping the hash encoding compact and avoiding repeated revalidation on internal hash calls. `Cache.logs` is not read by later execution and is already committed by the EVM accumulator hash through ordered receipts and logs bloom. Excluding it avoids redundant commitments while keeping the state hash aligned between `CacheDB<PanicDB>` and `State<TrieDB>`.

### Decision 7: Single-pass host execution with tracing

**Choice**: The `TracingEvmFactory` captures traces during the normal `build_block()` execution — one pass, not two.

**Rationale**: `execute_block()` builds a new block from transactions on top of current state. There's no prior result to "replay." The `TracingOpEvm` wrapper adds a single `.clone()` per transaction to capture `ResultAndState.state`. This runs only on the host (the guest uses the block cache), so the overhead is trivial relative to proving time.

### Decision 8: Chunk proofs cover only the block transaction body

**Choice**: Block-level prelude and epilogue execute outside chunk proving altogether. The chunk chain starts from the post-prelude flat state and ends at the post-last-transaction, pre-epilogue flat state.

**Rationale**: The current OP block executor applies once-per-block side effects outside the transaction loop. Replaying those effects in every chunk would break equivalence with monolithic execution. By moving them outside the chunk chain, chunk proofs become a clean proof of the ordered transaction body only, while the aggregation proof preserves exact once-per-block semantics.

### Decision 9: Chunk witnesses are boundary snapshots with full account metadata

**Choice**: For every address that a chunk may need, the witness carries the chunk-start `DbAccount` boundary snapshot (`AccountInfo`, `account_state`, and required storage slots), not only carried-forward storage values.

**Rationale**: Cross-chunk correctness depends on more than storage slots. Later chunks may depend on sender nonce/balance changes, account existence, code hash transitions, contract creation, and selfdestruct state from earlier chunks. revm's flat `Cache` can represent those transitions directly, so the witness construction algorithm should track chunk boundaries at the account level and prune only unused addresses or slots.

### Decision 10: The trace buffer represents only the ordered transaction body

**Choice**: The tracing buffer is defined as one entry per ordered block transaction in execution order. Prelude and epilogue state transitions are materialized separately as part of the once-per-block aggregation flow and are not appended to the chunk trace buffer. The host drains the shared tracing buffer via `TracingEvmFactory::take_traces()` at the end of each block execution, establishing an explicit per-block boundary.

**Rationale**: Chunk witnesses are built from tx-body boundaries. Keeping the trace buffer aligned to the ordered transaction list makes chunk partitioning deterministic and keeps `tx_start`/`tx_count` indexed against the same sequence used by the prover, guest, and aggregator. Because `KonaExecutor` clones the EVM factory into new builders, the drain step is necessary to prevent stale traces from prior block executions from accumulating in the shared `Arc<Mutex<Vec<EvmState>>>`.

### Decision 11: TracingOpEvm wraps any E: Evm via pure trait delegation

**Choice**: `TracingOpEvm<E: Evm>` wraps any `E: Evm` and implements the `Evm` trait by delegating all 8 required methods. A proven `CustomEvm` wrapper pattern exists in `op-reth/examples/custom-node/src/evm/alloy.rs` that follows this exact approach without `Deref`.

**Alternatives considered**:
- *Deref/DerefMut to OpContext*: Would work but is unnecessary. The block executor's field accesses (`self.evm.block()`, `self.evm.db_mut()`) resolve through `Evm` trait default methods (which call `components()`/`components_mut()`), not through `Deref`. Adding Deref ties TracingOpEvm to OpContext specifically, reducing generality.

**Rationale**: The `Evm` trait provides `db()` and `db_mut()` as defaults that delegate to `components()`/`components_mut()`. The block executor uses these trait methods. The wrapper only needs to implement the 8 required methods — 7 pure delegations plus `transact_raw()` with the state clone intercept. The `transact()` default method (which the block executor actually calls) internally calls `self.transact_raw()`, so our override is always invoked.

### Decision 12: Aggregation runs the standard executor pipeline via ChunkingEvm

**Choice**: The aggregation proof runs blocks through the standard `CachedExecutor` → `StatelessL2Builder::build_block()` → `OpBlockExecutor` pipeline, with `ChunkingEvmFactory` injected as the `EvmFactory`. `ChunkingEvm` returns pre-computed `ResultAndState` entries for tx-body `transact_raw()` calls while delegating `transact_system_call()` to the inner EVM for prelude and epilogue. The block executor commits each `ResultAndState.state` to `State<TrieDB>` through its normal path, accumulates receipts and gas. `finish()` applies the epilogue (balance increments) and returns `BlockExecutionResult` (receipts, gas_used, blob_gas_used). Then `build_block()` calls `state.merge_transitions()` + `state.take_bundle()` to produce the `BundleState`, and `seal_block()` constructs the header (state root, receipts root, EIP-1559 extra_data, etc.) — producing the final `BlockBuildingOutcome`. Hash chain verification and chunk receipt validation happen separately in `stitch_chunks()` (in the stitching layer) after `run_core_client()` returns.

**Alternatives considered**:
- *Manual aggregation with executor drop after prelude*: Apply prelude via `apply_pre_execution_changes()`, drop the executor, manually verify hash chain, materialize the tx-body transition from boundary snapshots, apply epilogue with standalone `post_block_balance_increments()` + `increment_balances()`, build the header manually using `compute_receipts_root()` / `ordered_trie_with_encoder()` and replicated `pub(crate)` EIP-1559 encoding (~40 lines). Rejected because it duplicates complex executor internals (receipt assembly, gas accounting, header construction, epilogue lifecycle), creates a maintenance surface with upstream hardfork changes, and requires manual `BundleState` construction with error-prone lifecycle semantics.
- *Hash-checking `post_tx_cache` without materializing it*: This leaves the live `State` at the post-prelude state, so the final trie root would omit all tx-body writes. Not applicable to the `ChunkingEvm` approach since the executor commits state changes directly.

**Rationale**: Because `ChunkingEvm::transact_raw()` returns the exact same `ResultAndState` that real EVM execution would produce (captured by `TracingEvmFactory` on the host), the block executor processes them indistinguishably from monolithic execution — committing state to `State<TrieDB>`, accumulating receipts, tracking gas used. The aggregation flow through `build_block()` is:
1. `CachedExecutor::new()` with `ChunkingEvmFactory` (keyed by block number)
2. `build_block()` creates `State`, EVM, and executor
3. `executor.execute_block(transactions)` — prelude runs via `transact_system_call()` → inner EVM; tx-body `transact()` → `transact_raw()` → `ChunkingEvm` returns pre-computed `ResultAndState`; `finish()` applies epilogue (balance increments) and returns `BlockExecutionResult` (receipts, gas_used, blob_gas_used)
4. `state.merge_transitions()` + `state.take_bundle()` — produces the `BundleState` from committed state changes
5. `seal_block()` constructs the header — computes `trie_db.state_root(&bundle)`, `receipts_root`, `logs_bloom`, EIP-1559 `extra_data` encoding, and all other header fields
6. `stitch_chunks()` verifies hash chain continuity and chunk receipts via `verify_stitching_journal()`

Note: `finish()` does NOT construct the header. It returns `(Evm, BlockExecutionResult)` — the epilogue (balance increments) and receipt/gas accumulation. Header construction is performed by `StatelessL2Builder::seal_block()`, called by `build_block()` after `execute_block()` returns. Because `seal_block()` is already part of the `build_block()` pipeline, it runs automatically — no replication of header assembly or EIP-1559 encoding logic is needed.

**Decision 12 addendum (chunk-side):** The chunk guest (`run_core_client` chunk branch) also runs transactions through upstream's `OpBlockExecutor`, but with three differences from aggregation:
1. `apply_pre_execution_changes` is skipped — the chunk witness already encodes post-prelude state (parent-hash ring buffer writes, beacon-root contract call, create2 deployer at the canyon transition). Part 8 aggregation binds `cw.cache` to the target block's post-prelude state.
2. `finish` is skipped — chunks do not apply the post-block balance increments. The aggregation proof owns that step.
3. The executor's `gas_used` / `da_footprint_used` / `receipts` accumulator fields (which are explicitly `pub` in upstream) are externally seeded from `witness.evm_state` so cross-chunk continuity holds. Without seeding, `execute_transaction_without_commit`'s block-available-gas and Jovian DA footprint prechecks would compare against a zero baseline, producing incorrect budget decisions for any chunk past the first.

A running `logs_bloom` is tracked separately in the chunk branch by OR-ing each committed receipt's logs into a local accumulator — `OpBlockExecutor` does not expose a per-block running bloom (the standard pipeline recomputes it at `seal_block` time from the final receipts vector).

This completes the symmetry across the two paths: both chunk proving and chunk aggregation inject behavior exclusively at layers above `OpBlockExecutor` — chunk proving via accumulator-field seeding + phase selection; aggregation via `ChunkingEvmFactory` — and neither re-implements per-transaction execution logic. The per-transaction validation (block-available-gas, Jovian DA footprint with L1Block preload), commit (deposit-nonce read with `unwrap_or_default`, receipt building via `OpAlloyReceiptBuilder`, state commit), and accumulator semantics are inherited directly from upstream and will automatically pick up future OP hardfork changes.

### Decision 13: Canonical state hashing normalizes CacheState and Cache to a common representation

**Choice**: Both the aggregation proof (which uses `State<TrieDB>` internally, storing `CacheState` with `CacheAccount`/`AccountStatus`) and chunk proofs (which use `CacheDB<PanicDB>`, storing `Cache` with `DbAccount`/`AccountState`) hash state through a shared canonical `Cache`-shaped encoding before SHA256. For state-backed execution, the hashed input is the effective flat state: the live `CacheState` plus `State.block_hashes`, and, when execution started from a preloaded `CacheDB` base cache, any untouched base-cache entries that remain logically part of state.

**Rationale**: `CacheState` uses `CacheAccount { account: Option<PlainAccount>, status: AccountStatus }` while `Cache` uses `DbAccount { info: AccountInfo, account_state: AccountState, storage }`. These contain the same logical data in different wrapper types. `AccountStatus` (richer enum: Loaded, Changed, Destroyed, DestroyedChanged, etc.) maps to `AccountState` (simpler: None, Touched, StorageCleared, NotExisting) for hashing purposes. Rather than defining custom normalization types, the implementation reuses revm's `AccountState` directly (via a public `normalize_account_status()` function for the `AccountStatus` → `AccountState` conversion) and operates directly on `DbAccount` instances — for `Cache`-backed views, existing `DbAccount` entries are used as-is; for `CacheState`-backed views, `DbAccount` instances are constructed from `CacheAccount`/`PlainAccount` data. Storage within each `DbAccount` (unordered `HashMap`) is sorted by slot key at encoding time. In the chunk guest, the logical post-state can be split between the underlying witness `Cache` in `CacheDB` and the live `CacheState` inside `State`, so post-state hashing must project the effective flat state rather than bare `CacheState` alone. The canonical encoding is defined over `DbAccount` fields (nonce, balance, code_hash, account_state, storage), contract code_hashes only, and block_hashes — while cached bytecode from untrusted witness data is validated once at witness-cache ingestion before first hash/execution use. This keeps both representations in the same hash domain without weakening authentication of executed code. A streaming SHA256 hasher is used to avoid intermediate buffer allocation.

### Decision 14: Cache serialization uses rkyv-compatible mirror types

**Choice**: Define rkyv-serializable mirror types (`SerializableCache`, `SerializableDbAccount`, etc.) in Kailua code that can be constructed from revm's `Cache`/`CacheState` and converted back. Use these for chunk witness serialization within the `Witness` struct.

**Alternatives considered**:
- *Enable serde on revm types and use bincode*: Breaks consistency with the rest of the Witness struct which uses rkyv. Mixed serialization formats add complexity.
- *Implement rkyv traits on foreign revm types*: Orphan rule prevents this without newtype wrappers.

**Rationale**: revm's `Cache`, `DbAccount`, `AccountInfo`, and `Bytecode` derive `serde::{Serialize, Deserialize}` (feature-gated) but have no rkyv support. Since the existing `Witness` struct uses rkyv throughout, mirror types maintain consistency. The conversion is mechanical (field-by-field copy) and tested via round-trip tests.

### Decision 15: BundleState lifecycle correctness is inherited from pre-computed ResultAndState

**Choice**: The aggregation proof builds its `BundleState` naturally through the standard executor commit path. Each pre-computed `ResultAndState` (returned by `ChunkingEvm::transact_raw()`) carries the same `EvmState` (per-account `Account` with `AccountStatus` flags and storage slots) that monolithic execution would produce. The executor's `State<TrieDB>::commit()` translates these into `BundleAccount` entries with correct lifecycle semantics — creation, destruction, storage-wipe, and destroy-and-recreate cases are all handled identically to monolithic execution.

**Alternatives considered**:
- *Manually construct BundleState from boundary snapshots*: Diff the post-prelude and post-transaction flat states, derive `BundleAccount` statuses from both snapshots plus `account_state`. Rejected because it requires reimplementing the executor's lifecycle translation logic, which is error-prone for edge cases (selfdestruct + recreate, storage wipe with zero-valued slots needed for trie correctness).

**Rationale**: `trie_db.state_root(&bundle)` depends on `BundleAccount` lifecycle semantics to decide when to delete account leaves and how to treat wiped storage. Because `ChunkingEvm` returns the identical `ResultAndState` captured by `TracingEvmFactory` during host-side execution, and the executor processes these through its standard commit path, the lifecycle semantics are correct by construction. No manual translation or snapshot diffing is required.

### Decision 16: State.cache is lazy — post-state hash requires overlay onto CacheDB base

**Choice**: When computing `post_db_hash` after chunk execution on `State<CacheDB<PanicDB>>`, the hash is taken over the effective flat state: `CacheDB.cache` (the pre-loaded witness, which is never mutated during execution) overlaid with `State.cache` (which contains all accounts/slots that were accessed — both read-only and modified). Unaccessed witness entries in `CacheDB.cache` remain part of the committed state. This overlay is lifecycle-aware rather than a fieldwise merge: if the live normalized account state implies deletion or storage wipe, inherited base info/storage are discarded before hashing.

**Rationale**: `State<DB>` starts with an empty `CacheState` and lazily loads entries from the underlying `CacheDB` on first access. After execution, `State.cache` contains all *accessed* accounts (both modified and read-only, with `AccountStatus::Loaded` vs `AccountStatus::Changed` distinguishing them). But accounts pre-loaded in `CacheDB.cache` that were *never accessed* by any transaction are only in `CacheDB.cache`. Hashing `State.cache` alone would miss them. The overlay recipe is: start with `CacheDB.cache`, then for each entry in `State.cache`, overwrite the corresponding `CacheDB.cache` entry with the `State.cache` version (which reflects modifications). When the live normalized account state is `StorageCleared`, the overlay must rebuild that `DbAccount` from the live state and drop inherited base storage; when the live account is deleted / non-existent, the overlay must drop inherited base info and storage entirely. This produces the complete post-execution state without retaining stale pre-state fields.

### Decision 17: Block hashes populated in CacheDB.cache — State loads lazily

**Choice**: For chunk witnesses, block hashes are stored in `CacheDB.cache.block_hashes`. `State.block_hashes` (a separate `BTreeMap`) starts empty and lazily loads from `CacheDB` when `BLOCKHASH` opcodes execute. The canonical state hash includes block hashes from the overlay (same as accounts: CacheDB base + State overrides).

**Rationale**: `State` and `CacheDB` maintain independent block hash caches. `State.block_hash(number)` checks `State.block_hashes` first, then calls `self.database.block_hash(number)` (which goes to `CacheDB.cache.block_hashes`, then `PanicDB`). Populating `CacheDB.cache.block_hashes` is sufficient — `State` will load them lazily. The canonical hash must account for both sources in the overlay.

### Decision 18: Header construction is handled by the standard build_block pipeline (no replication needed)

**Choice**: The aggregation proof runs through `StatelessL2Builder::build_block()`, which calls `executor.execute_block()` followed by `seal_block()`. Header construction — including `trie_db.state_root(&bundle)`, `compute_receipts_root()`, `logs_bloom()`, and hardfork-specific `extra_data` encoding via the `pub(crate)` functions `encode_holocene_eip_1559_params` / `encode_jovian_eip_1559_params` — is performed by `seal_block()` as part of the standard pipeline. Because `ChunkingEvm` makes the executor's `execute_block()` produce a correct `BlockExecutionResult` (with accumulated receipts, gas, and state changes), `seal_block()` receives valid inputs and constructs the header correctly.

**Alternatives considered**:
- *Build the header manually outside `build_block()`*: Replicate `seal_block()`'s logic (~80+ lines including `pub(crate)` EIP-1559 encoding) in the aggregation proof. Rejected because this duplicates vendored logic, creates a maintenance surface that must track upstream hardfork changes (Holocene, Jovian, and future forks), and is unnecessary since the `ChunkingEvm` approach enables the standard `build_block()` pipeline to run end-to-end.

**Rationale**: `seal_block()` uses `BlockExecutionResult` (receipts, gas_used, blob_gas_used) from `execute_block()` plus `BundleState` from `state.take_bundle()` to compute all header fields. With `ChunkingEvm`, the executor accumulates these correctly from pre-computed `ResultAndState` entries. The `pub(crate)` EIP-1559 functions (`encode_holocene_eip_1559_params`, `encode_jovian_eip_1559_params`) are already called within `seal_block()` — they do not need replication. This eliminates the entire header-construction maintenance surface.

### Decision 19: EvmFactory injection is sufficient — OpBlockExecutorFactory and OpBlockExecutor require no modification

**Choice**: The chunking system injects custom behavior exclusively at the `EvmFactory` level (`TracingEvmFactory`, `ChunkingEvmFactory`). The upstream `OpBlockExecutorFactory<R, Spec, EvmFactory>` and `OpBlockExecutor` are used unchanged.

**Alternatives considered**:
- *Wrap or extend `OpBlockExecutorFactory`*: Would provide a higher-level interception point (e.g., hooking `apply_pre_execution_changes` or `finish`). Unnecessary because the only interception needed is `transact_raw()` on individual transactions — both tracing (host) and aggregation (guest) inject behavior exclusively at that method.
- *Wrap `OpBlockExecutor` directly*: Would give access to per-block lifecycle hooks. Rejected because the `ChunkingEvm` approach (Decision 12) runs prelude and epilogue through the standard executor via `transact_system_call()` delegation — no executor-level hooks are needed.

**Rationale**: `OpBlockExecutorFactory` (in `alloy-op-evm/src/block/mod.rs`) is generic over its `EvmFactory` parameter — it holds the factory and passes it through to `OpBlockExecutor` when creating execution environments. `StatelessL2Builder` constructs an `OpBlockExecutorFactory::new(receipt_builder, config, evm_factory)` from whatever `EvmFactory` it receives. Because the generic plumbing in the vendored stack (`StatelessL2Builder<P, H, Evm>` → `OpBlockExecutorFactory<R, Spec, EvmFactory>` → `OpBlockExecutor` → EVM creation) is fully parameterized, swapping `OpEvmFactory` for `TracingEvmFactory` or `ChunkingEvmFactory` at `CachedExecutor::new()` propagates automatically through the entire chain. The block executor's field accesses (`self.evm.block()`, `self.evm.db_mut()`) resolve through `Evm` trait default methods, and the `transact()` default internally calls `self.transact_raw()`, so the tracing/chunking intercept is always invoked without any changes above the `EvmFactory` level. This is consistent with Decision 12: the aggregation proof runs the full standard executor pipeline, with `ChunkingEvm` injected at the `EvmFactory` level as the only modification point.

This invariant is upheld by **both** paths. Aggregation (Decision 12) injects at the `EvmFactory` level via `ChunkingEvmFactory`. Chunk proving (Decision 12 addendum) constructs `OpBlockExecutor::new(...)` directly with the standard `OpEvmFactory`, seeds its `pub` accumulator fields (`gas_used`, `da_footprint_used`, `receipts`) from `witness.evm_state`, and drives per-tx execution by calling `execute_transaction_without_commit` and `commit_transaction` externally while omitting `apply_pre_execution_changes` and `finish`. In both cases, `OpBlockExecutor`'s per-transaction validation (block-available-gas, Jovian DA footprint with explicit L1Block preload), commit (deposit-nonce read, receipt building via `OpAlloyReceiptBuilder`, state commit), and accumulator semantics are reused byte-for-byte. No manual mirror of these rules exists in Kailua code, so upstream hardfork changes (Isthmus, Interop, future forks) are auto-inherited on both paths.

## Risks / Trade-offs

**[Risk] Chunk witness size may exceed monolithic witness for overlapping state access** → Mitigation: The total witness across all chunks may duplicate state that multiple chunks read. This is acceptable per design constraints. The `max_txs_per_chunk` parameter allows tuning the parallelism/witness-size trade-off.

**[Risk] Memory DB hashing adds overhead in the guest** → Mitigation: SHA256 of the Cache is O(n) in the number of accounts/slots. For typical blocks this is small. The hash runs twice per chunk (pre and post) — a constant cost compared to EVM execution.

**[Risk] PanicDB hides host-side witness construction bugs** → Mitigation: Extensive testing of the chunk witness construction algorithm. The panic is a hard failure, not silent corruption. Integration tests will verify that chunk proofs succeed for known blocks.

**[Risk] CacheDB serialization for chunk witnesses** → Mitigation: revm's `CacheDB` may not have built-in serialization. We'll need to implement `rkyv::Serialize`/`Deserialize` for the Cache structure or use a custom serialization wrapper. This is mechanical but must be tested for round-trip correctness.

**[Risk] EVM state accumulator completeness** → Mitigation: Must identify ALL cross-chunk accumulators (gas, DA footprint, blob gas, logs bloom, receipts). Missing an accumulator breaks block header construction. The EVM state struct will be exhaustively validated against `OpBlockExecutor`'s internal state.

**[Trade-off] Double execution on host for blocks with chunking** → The `TracingEvmFactory` runs the block once with tracing. This is the same cost as today's monolithic pre-execution, plus the `.clone()` overhead per transaction. No additional execution pass.

**[Trade-off] Increased proof count** → A block with N chunks produces N+1 proofs (N chunks + 1 aggregation) instead of 1. Proof composition via `env::verify()` adds recursive verification overhead. This is acceptable because the chunk proofs run in parallel, reducing wall-clock time.

**[Risk] CacheState/Cache representation mismatch** → Mitigation: The aggregation proof uses `State<TrieDB>` (CacheState/CacheAccount/AccountStatus) while chunk proofs use `CacheDB<PanicDB>` (Cache/DbAccount/AccountState). A canonical normalization layer maps both to the same byte encoding before hashing; when a hidden preloaded `CacheDB` base exists, the post-state hash is taken over the effective flat state formed by overlaying the live `CacheState` projection onto that base cache. This normalization is HASH-ONLY — the richer lifecycle semantics are preserved automatically by the standard executor commit path (Decision 15). Round-trip tests must verify hash equivalence for identical logical state across both representations.

**[Risk] Pre-computed ResultAndState must exactly match monolithic execution** → Mitigation: `TracingEvmFactory` captures the full `ResultAndState` from real EVM execution via `transact_raw()` cloning. The `ChunkingEvm` returns these unmodified. Since the same `ResultAndState` flows through the standard executor commit path, lifecycle semantics (creation, selfdestruct, storage wipe) are handled identically to monolithic execution. Integration tests must verify that chunk-aggregated blocks produce identical `BlockBuildingOutcome` (state root, receipts root, gas used) to monolithic execution.

**[Risk] rkyv mirror types for Cache must stay synchronized** → Mitigation: Mirror types (`SerializableCache`, etc.) must exactly match revm's `Cache` field layout. revm version upgrades that change `Cache`, `DbAccount`, or `AccountInfo` fields will require mirror type updates. Pin revm version and add compile-time assertions where possible.

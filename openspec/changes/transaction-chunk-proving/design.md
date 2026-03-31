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

### Decision 6: Memory DB hash covers the complete Cache structure

**Choice**: The deterministic SHA256 hash includes accounts (info + storage + account_state), contracts (code_hash + bytecode), and block_hashes. Nothing is excluded from the hash.

**Rationale**: Assuming any field is irrelevant creates a subtle attack surface. The full Cache hash is the only safe commitment.

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

**Choice**: The tracing buffer is defined as one entry per ordered block transaction in execution order. Prelude and epilogue state transitions are materialized separately as part of the once-per-block aggregation flow and are not appended to the chunk trace buffer.

**Rationale**: Chunk witnesses are built from tx-body boundaries. Keeping the trace buffer aligned to the ordered transaction list makes chunk partitioning deterministic and keeps `tx_start`/`tx_count` indexed against the same sequence used by the prover, guest, and aggregator.

### Decision 11: TracingOpEvm must implement Deref/DerefMut to OpContext

**Choice**: `TracingOpEvm<DB, I>` implements `Deref<Target = OpContext<DB>>` and `DerefMut` by delegating to `self.inner.ctx()` / `self.inner.ctx_mut()`, which are public inherent methods on `OpEvm`.

**Rationale**: `OpBlockExecutor` accesses EVM fields through `Deref`, not through the `Evm` trait. Every access like `self.evm.block()`, `self.evm.db_mut()`, `self.evm.cfg` resolves via `Deref<Target = OpContext<DB>>`. Without this impl, the block executor cannot access block environment or database state. This is a hidden requirement not visible from the `Evm` trait definition alone.

### Decision 12: Aggregation prelude/epilogue uses public BlockExecutor and standalone functions

**Choice**: The aggregation proof applies prelude via the public `BlockExecutor::apply_pre_execution_changes()` trait method, then drops the executor to release the state borrow. After chunk verification and state application, epilogue is applied manually using the public standalone functions `post_block_balance_increments()` and `State::increment_balances()`.

**Alternatives considered**:
- *Using `finish()` for epilogue*: `finish()` bundles balance increments with receipt/gas assembly and expects the executor to have processed transactions. Since the aggregation proof verifies chunks instead of executing transactions, the executor's internal receipt list is empty, making `finish()` produce incorrect results.
- *Calling `apply_post_execution_changes()` (trait method)*: This is a separate post-execution system call hook from alloy-evm's base trait. Currently a no-op for OP-Stack blocks but may gain future hardfork logic. Should be called for forward compatibility.

**Rationale**: `apply_pre_execution_changes()` is already used independently in op-reth's payload builder and flashblocks worker. The balance increment functions are public imports from `alloy_evm::block::state_changes`. This gives us clean prelude/epilogue separation without modifying vendored code. The aggregation flow becomes:
1. Create executor → `apply_pre_execution_changes()` (prelude)
2. Drop executor (release state borrow)
3. Hash post-prelude state → `initial_db_hash`
4. Verify chunk hash chain
5. Load final state from witness, apply diff to State
6. Apply `post_block_balance_increments()` + `increment_balances()` (epilogue)
7. `state.merge_transitions()` → `take_bundle()` → `trie_db.state_root(&bundle)`
8. Build header manually using public `compute_receipts_root()`, `ordered_trie_with_encoder()`

### Decision 13: Canonical state hashing normalizes CacheState and Cache to a common representation

**Choice**: Both the aggregation proof (which uses `State<TrieDB>` internally, storing `CacheState` with `CacheAccount`/`AccountStatus`) and chunk proofs (which use `CacheDB<PanicDB>`, storing `Cache` with `DbAccount`/`AccountState`) hash state through a shared canonical encoding that normalizes both representations to the same byte layout before SHA256.

**Rationale**: `CacheState` uses `CacheAccount { account: Option<PlainAccount>, status: AccountStatus }` while `Cache` uses `DbAccount { info: AccountInfo, account_state: AccountState, storage }`. These contain the same logical data in different wrapper types. `AccountStatus` (richer enum: Loaded, Changed, Destroyed, DestroyedChanged, etc.) maps to `AccountState` (simpler: None, Touched, StorageCleared, NotExisting) for hashing purposes. The canonical encoding is defined over logical account data (nonce, balance, code_hash, existence, storage) so both representations produce identical hashes for the same logical state.

### Decision 14: Cache serialization uses rkyv-compatible mirror types

**Choice**: Define rkyv-serializable mirror types (`SerializableCache`, `SerializableDbAccount`, etc.) in Kailua code that can be constructed from revm's `Cache`/`CacheState` and converted back. Use these for chunk witness serialization within the `Witness` struct.

**Alternatives considered**:
- *Enable serde on revm types and use bincode*: Breaks consistency with the rest of the Witness struct which uses rkyv. Mixed serialization formats add complexity.
- *Implement rkyv traits on foreign revm types*: Orphan rule prevents this without newtype wrappers.

**Rationale**: revm's `Cache`, `DbAccount`, `AccountInfo`, and `Bytecode` derive `serde::{Serialize, Deserialize}` (feature-gated) but have no rkyv support. Since the existing `Witness` struct uses rkyv throughout, mirror types maintain consistency. The conversion is mechanical (field-by-field copy) and tested via round-trip tests.

### Decision 15: BundleState constructed from Cache diff with empty reverts

**Choice**: The aggregation proof constructs a `BundleState` by diffing the initial (post-prelude) and final (post-chunks + epilogue) Cache snapshots. For each changed account: `BundleAccount { info: final_info, original_info: initial_info, storage: changed_slots, status: Changed }`. Reverts are left empty.

**Rationale**: `trie_db.state_root(&bundle)` only reads forward state (`bundle.state()`, `account.info`, `storage.present_value`). It does not access `reverts`. Empty reverts are safe for state root computation. This avoids the complexity of constructing per-transaction revert records.

### Decision 16: Header construction replicates EIP-1559 extra_data encoding

**Choice**: The aggregation proof builds the block header manually using public APIs (`compute_receipts_root()`, `ordered_trie_with_encoder()`, `trie_db.state_root()`, `trie_db.get_trie_account()` for withdrawal root, `alloy_primitives::logs_bloom()`). The `extra_data` field requires replicating the `pub(crate)` functions `encode_holocene_eip_1559_params` and `encode_jovian_eip_1559_params` (~40 lines).

**Rationale**: All header fields except `extra_data` can be computed from public APIs. The EIP-1559 encoding functions are `pub(crate)` in kona-executor, requiring ~40 lines of replication. This is the only vendored logic that must be mirrored. It must be kept in sync with upstream hardfork changes — a maintenance cost accepted as the price of zero vendored modifications.

## Risks / Trade-offs

**[Risk] Chunk witness size may exceed monolithic witness for overlapping state access** → Mitigation: The total witness across all chunks may duplicate state that multiple chunks read. This is acceptable per design constraints. The `max_txs_per_chunk` parameter allows tuning the parallelism/witness-size trade-off.

**[Risk] Memory DB hashing adds overhead in the guest** → Mitigation: SHA256 of the Cache is O(n) in the number of accounts/slots. For typical blocks this is small. The hash runs twice per chunk (pre and post) — a constant cost compared to EVM execution.

**[Risk] PanicDB hides host-side witness construction bugs** → Mitigation: Extensive testing of the chunk witness construction algorithm. The panic is a hard failure, not silent corruption. Integration tests will verify that chunk proofs succeed for known blocks.

**[Risk] CacheDB serialization for chunk witnesses** → Mitigation: revm's `CacheDB` may not have built-in serialization. We'll need to implement `rkyv::Serialize`/`Deserialize` for the Cache structure or use a custom serialization wrapper. This is mechanical but must be tested for round-trip correctness.

**[Risk] EVM state accumulator completeness** → Mitigation: Must identify ALL cross-chunk accumulators (gas, DA footprint, blob gas, logs bloom, receipts). Missing an accumulator breaks block header construction. The EVM state struct will be exhaustively validated against `OpBlockExecutor`'s internal state.

**[Trade-off] Double execution on host for blocks with chunking** → The `TracingEvmFactory` runs the block once with tracing. This is the same cost as today's monolithic pre-execution, plus the `.clone()` overhead per transaction. No additional execution pass.

**[Trade-off] Increased proof count** → A block with N chunks produces N+1 proofs (N chunks + 1 aggregation) instead of 1. Proof composition via `env::verify()` adds recursive verification overhead. This is acceptable because the chunk proofs run in parallel, reducing wall-clock time.

**[Risk] CacheState/Cache representation mismatch** → Mitigation: The aggregation proof uses `State<TrieDB>` (CacheState/CacheAccount/AccountStatus) while chunk proofs use `CacheDB<PanicDB>` (Cache/DbAccount/AccountState). A canonical normalization layer maps both to the same byte encoding before hashing. Round-trip tests must verify hash equivalence for identical logical state across both representations.

**[Risk] EIP-1559 extra_data encoding drift** → Mitigation: ~40 lines of EIP-1559 encoding logic replicated from `pub(crate)` kona-executor utils. Must be updated when upstream adds new hardfork-specific encoding. Tracked as a known maintenance surface.

**[Risk] `apply_post_execution_changes()` may gain future logic** → Mitigation: Currently a no-op for OP-Stack but is a public trait method on `BlockExecutor`. The aggregation flow should call it (via the executor, before dropping it) for forward compatibility with future hardforks that add post-execution system calls.

**[Risk] rkyv mirror types for Cache must stay synchronized** → Mitigation: Mirror types (`SerializableCache`, etc.) must exactly match revm's `Cache` field layout. revm version upgrades that change `Cache`, `DbAccount`, or `AccountInfo` fields will require mirror type updates. Pin revm version and add compile-time assertions where possible.

### Requirement: `CachedEvm<E: Evm>` is a single `Evm`-trait wrapper that unifies tracing, caching, and prestate authentication

The system SHALL provide one `Evm`-trait wrapper — `CachedEvm<E: Evm>` — in `kailua-kona` at `crates/kona/src/evm/cached.rs` that serves three roles depending on how it was constructed:

1. **Pure pass-through.** With an empty partial cache and no collector, every `transact_raw` delegates to the inner EVM. Equivalent to not wrapping at all.
2. **Trace capture.** With an empty cache and a shared `TransactionResultCollector`, every successful fall-through `transact_raw` captures `expected_state` BEFORE delegating, then on `Ok` appends `PartialExecutionTrace { tx_hash: keccak256(tx.enveloped_tx), result: PartialResultAndState::from(r.clone()), expected_state }` to the last inner `Vec` of the collector. This replaces the former standalone `TracingOpEvm`.
3. **Cached serve with prestate authentication.** With a seeded `Vec<PartialExecution>` (reversed, wrapped in `ActivePartialExecution { partial, expected_state_verified: false }`, so `pop()` yields execution-order entries) each `transact_raw(tx)` first checks whether the top partial's next `tx_hashes.last()` equals the incoming tx's identity hash. On match it (a) asserts `chunk.partial.block_env == self.evm.block()`, (b) on the FIRST cached serve in this partial, asserts `capture_required_expected_state(db) == chunk.partial.expected_state` and flips `expected_state_verified = true` (subsequent cached serves skip this check), (c) for every `(addr, account)` in the cached state performs a live-DB read and asserts `stored_info == account.original_info` (or, if `None`, that the account is `Created` / `LoadedAsNotExisting`), then for every `(slot, evm_slot)` asserts `db.storage(addr, slot) == evm_slot.original_value`, and (d) returns the cached result. On mismatch it falls through to the inner EVM and (if a collector is attached) records the fresh capture. This replaces the former standalone `ChunkingEvm`.

The inner EVM is required to satisfy `Evm<HaltReason = OpHaltReason, Tx = OpTransaction<TxEnv>>` with `E::DB: revm::Database`. The wrapper relays all other `Evm` trait methods (`block`, `chain_id`, `finish`, `set_inspector_enabled`, `components`, `components_mut`, `transact_system_call`) transparently to the inner EVM — in particular `transact_system_call` NEVER consumes cached entries and NEVER appends to the trace collector.

#### Scenario: empty cache + no collector is a transparent pass-through
- **WHEN** `CachedEvm::new_with_traces(inner, Vec::new(), None)` is constructed and executed
- **THEN** every `Evm` method, including `transact_raw`, behaves identically to calling the method on `inner`

#### Scenario: fall-through captures go to the collector's last slot
- **WHEN** `CachedEvm::transact_raw(tx)` falls through successfully and a collector is attached
- **THEN** exactly one `PartialExecutionTrace { tx_hash, result, expected_state }` is appended to the last inner `Vec` of the collector, where `expected_state` is the snapshot captured BEFORE the fall-through delegation

#### Scenario: cached entry serves without invoking the inner EVM
- **WHEN** the top partial's next `tx_hashes.last()` matches the incoming hash and the prestate / expected-state authentication succeeds
- **THEN** the cached `ResultAndState` is returned and `self.evm.transact_raw(tx)` is NOT called

#### Scenario: exhausted partials are peeled off before matching
- **WHEN** the top partial has `results.is_empty()`
- **THEN** it is popped from the cache before the match check, so the next partial becomes active

#### Scenario: `transact_system_call` is transparent
- **WHEN** the block executor invokes `transact_system_call` for block-level prelude / epilogue work
- **THEN** the call goes to `self.evm.transact_system_call` unchanged and never advances the partial cursor nor appends to the collector

### Requirement: `ActivePartialExecution` gates the once-per-partial expected-state check

The system SHALL define `ActivePartialExecution { partial: PartialExecution, expected_state_verified: bool }` (in `crates/kona/src/evm/partial.rs`) wrapping each `PartialExecution` inside `CachedEvm.cache`. The `expected_state_verified` flag MUST start `false` and SHALL flip to `true` on the first successful cached serve in this partial — gating the `capture_required_expected_state(db) == chunk.partial.expected_state` assertion.

#### Scenario: first cached serve runs the expected-state check
- **WHEN** the first matching cached entry in a partial is being served
- **THEN** the wrapper calls `capture_required_expected_state(db)` and asserts equality with `chunk.partial.expected_state`

#### Scenario: subsequent cached serves skip the expected-state check
- **WHEN** the partial has at least one prior cached serve and `expected_state_verified == true`
- **THEN** the wrapper does not call `capture_required_expected_state(db)` again, even if intervening `db.commit`s have updated the L1Block slots — only the per-tx prestate check runs

### Requirement: `CachedEvmFactory` produces `CachedEvm` instances and drives per-EVM trace slots

The system SHALL provide a `CachedEvmFactory` struct in `kailua-kona` at `crates/kona/src/evm/cached.rs` with:

- `inner: OpEvmFactory` — the real factory that builds the underlying `OpEvm`.
- `cache: Arc<Mutex<Vec<Vec<PartialExecution>>>>` — per-block partials, stored in REVERSE execution order so `pop()` yields blocks in ascending execution order. Constructors reverse caller-supplied vecs.
- `block_traces: Option<TransactionResultCollector>` — an optional `Arc<Mutex<Vec<Vec<PartialExecutionTrace>>>>`. Outer index = EVM creation order.

Two constructors:

- `new(cache) -> Self` — no trace collector.
- `new_with_traces(cache, block_traces) -> Self` — attaches the collector.

The factory implements `EvmFactory` with the OP-specific associated types (`Context<DB> = OpContext<DB>`, `Tx = OpTransaction<TxEnv>`, `Error = EVMError<_, OpTxError>`, `HaltReason = OpHaltReason`, `Spec = OpSpecId`, `BlockEnv = BlockEnv`, `Precompiles = PrecompilesMap`). Both `create_evm` and `create_evm_with_inspector` SHALL:

1. Pop the next block's partials via `take_next_chunks()` (empty if exhausted).
2. Push an empty slot onto the trace collector via `push_trace_slot()` so the new `CachedEvm`'s captures land in a fresh per-EVM `Vec`.
3. Delegate to `inner.create_evm(db, input)` (or `create_evm_with_inspector(db, input, inspector)`).
4. Wrap the result via `CachedEvm::new_with_traces(inner_evm, popped_partials, block_traces.clone())`.

A helper `take_all_block_traces() -> Vec<Vec<PartialExecutionTrace>>` SHALL atomically drain the collector (returning an empty vec when none is attached).

#### Scenario: `create_evm` consumes partials positionally
- **WHEN** a factory is constructed with `[[a, b], [], [c]]` and `create_evm` is called three times in order
- **THEN** the first `CachedEvm` holds `[a, b]`, the second holds `[]`, and the third holds `[c]`

#### Scenario: `push_trace_slot` per EVM
- **WHEN** `create_evm` is called and a collector is attached
- **THEN** an empty `Vec` is appended to the collector BEFORE the new `CachedEvm` runs any transaction, so that EVM's first capture lands in a fresh, empty slot

#### Scenario: `take_all_block_traces` drains atomically
- **WHEN** two successful fall-through transactions ran in EVM A and three in EVM B, then `take_all_block_traces()` is called
- **THEN** the returned `Vec<Vec<_>>` has exactly two inner `Vec`s, with A's two captures then B's three captures, and the shared collector is now empty

#### Scenario: empty cache factory matches `OpEvmFactory`
- **WHEN** `CachedEvmFactory::new(Vec::new())` produces an EVM and drives it through a full block execution
- **THEN** the resulting block outcome is byte-identical to one produced by `OpEvmFactory::default()`

### Requirement: `CachedExecutor` is generic over any `EvmFactory`

`CachedExecutor::new()` in `kailua-kona` at `crates/kona/src/executor.rs` SHALL accept a generic `Evm: EvmFactory<Spec = OpSpecId, BlockEnv = BlockEnv> + Send + Sync + Clone + Debug + 'static` parameter (with the additional `FromTxWithEncoded<OpTxEnvelope>`, `FromRecoveredTx<OpTxEnvelope>`, and `OpTxEnv` bounds on `Evm::Tx` required by `KonaExecutor`) rather than hardcoding `OpEvmFactory`. The factory is passed through to `KonaExecutor::new(rollup_config, trie_provider, trie_hinter, evm_factory, None)`. All existing `CachedExecutor` behavior (cache hit precedence, collection target, pop-reversal) is preserved — chunk logic lives entirely inside the EVM layer.

Callers in `run_core_client` always provide `CachedEvmFactory::new_with_traces(partial_executions, partials_collector)`, collapsing what were formerly separate "tracing" and "aggregation" code paths into a single executor construction.

#### Scenario: `CachedExecutor` composes with arbitrary `EvmFactory`
- **WHEN** a caller constructs `CachedExecutor::new(cache, config, trie_provider, trie_hinter, evm_factory, None)` for any `EvmFactory` matching the trait bounds
- **THEN** the executor's behavior is identical to the pre-generic version — confirming the generalization is a pure refactor with no behavior change

#### Scenario: partial-proving and aggregation share one code path
- **WHEN** `run_core_client` constructs its executor in either the EXECUTION ONLY or DERIVATION & EXECUTION branch
- **THEN** it uses `CachedEvmFactory::new_with_traces(partial_executions, partials_collector)` — emitting a `CachedEvm`-backed executor that handles empty-partials, trace-capture, and cached-serve uniformly

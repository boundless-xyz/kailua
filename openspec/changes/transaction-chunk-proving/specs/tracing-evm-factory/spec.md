## ADDED Requirements

### Requirement: TracingOpEvm wraps an inner Evm and captures tx-body EvmState

**Crate:** `kailua-prover` (host-only types in `crates/prover/src/evm.rs`)

The system SHALL provide a generic `TracingOpEvm<E: Evm>` struct that wraps an inner `E: Evm` and implements the `Evm` trait. In this change, `TracingEvmFactory` instantiates it as `TracingOpEvm<OpEvm<DB, I, PrecompilesMap>>`. The `transact_raw()` method SHALL clone the full `ResultAndState` into a shared trace buffer before returning. The trace buffer type is `Vec<ResultAndState>`, capturing both the `ExecutionResult` (gas, logs, output, status) and the `EvmState` (per-tx state diff). The `EvmState` component is used for chunk witness cache construction; the full `ResultAndState` is used for constructing `Chunk.results` for aggregation. The trace buffer represents only the ordered block transaction body used for chunk proving. Block-level prelude and epilogue state transitions are handled separately and SHALL NOT append extra entries to the chunk trace buffer. All other `Evm` trait methods SHALL delegate to the inner EVM without modification.

#### Scenario: transact_raw captures full ResultAndState on success
- **WHEN** `TracingOpEvm::transact_raw(tx)` is called and the inner EVM returns `Ok(ResultAndState { result, state })`
- **THEN** the full `ResultAndState` is cloned and appended to the shared trace buffer, and the original `Ok(ResultAndState)` is returned unmodified

#### Scenario: transact_raw propagates errors without capturing
- **WHEN** `TracingOpEvm::transact_raw(tx)` is called and the inner EVM returns `Err(e)`
- **THEN** the error is returned unmodified and no entry is added to the trace buffer

#### Scenario: transact_system_call does not append tx-body traces
- **WHEN** `TracingOpEvm::transact_system_call(...)` is called for block-level prelude or epilogue work
- **THEN** the call is delegated to the inner `OpEvm`, and no entry is appended to the chunk trace buffer

#### Scenario: delegation methods are transparent
- **WHEN** any `Evm` trait method other than `transact_raw()` is called on `TracingOpEvm`
- **THEN** the call is delegated to the inner `OpEvm` and the result is returned unmodified

### Requirement: TracingOpEvm uses pure Evm trait delegation (no Deref required)

`TracingOpEvm<E: Evm>` SHALL wrap any `E: Evm` and implement the `Evm` trait by delegating all 8 required methods. The block executor accesses `self.evm.block()` and `self.evm.db_mut()` through `Evm` trait default methods (which call `components()`/`components_mut()`), not through `Deref`. This follows the proven `CustomEvm` wrapper pattern in `op-reth/examples/custom-node/src/evm/alloy.rs`.

#### Scenario: block executor field access works via trait methods
- **WHEN** `OpBlockExecutor` accesses `self.evm.block()` or `self.evm.db_mut()` on a `TracingOpEvm`
- **THEN** the access resolves through the `Evm` trait's `block()` and `db_mut()` default methods, which delegate to `components()`/`components_mut()` on the inner EVM

#### Scenario: transact() calls transact_raw() through trait default
- **WHEN** `OpBlockExecutor` calls `self.evm.transact(tx_env)` (the provided default method)
- **THEN** it internally calls `self.transact_raw(tx.into_tx_env())`, which dispatches to `TracingOpEvm::transact_raw()` and captures the state

### Requirement: TracingEvmFactory wraps OpEvmFactory and injects TracingOpEvm

The system SHALL provide a `TracingEvmFactory` struct that implements `EvmFactory` with `type Evm<DB, I> = TracingOpEvm<OpEvm<DB, I, PrecompilesMap>>`. The factory SHALL hold a shared `Arc<Mutex<Vec<EvmState>>>` trace buffer that is passed to each `TracingOpEvm` it creates, and expose a `take_traces()` helper that atomically drains that shared buffer at the end of each block execution.

#### Scenario: create_evm produces TracingOpEvm
- **WHEN** `TracingEvmFactory::create_evm(db, env)` is called
- **THEN** it creates an `OpEvm` via the inner `OpEvmFactory::create_evm(db, env)` and wraps it in `TracingOpEvm` with the shared trace buffer

#### Scenario: create_evm_with_inspector produces TracingOpEvm
- **WHEN** `TracingEvmFactory::create_evm_with_inspector(db, env, inspector)` is called
- **THEN** it creates an `OpEvm` via the inner `OpEvmFactory::create_evm_with_inspector(db, env, inspector)` and wraps it in `TracingOpEvm` with the shared trace buffer

#### Scenario: trace buffer accumulates across tx-body transactions within a block
- **WHEN** a block with N transactions is executed using a `TracingEvmFactory`-produced EVM
- **THEN** the trace buffer contains exactly N entries, one per ordered block transaction, in execution order, and block-level prelude / epilogue work does not add extra entries

#### Scenario: take_traces establishes a per-block boundary
- **WHEN** block execution finishes and the host calls `TracingEvmFactory::take_traces()`
- **THEN** it receives exactly the traces accumulated for that block and the shared buffer is reset to empty before the next block execution begins

### Requirement: CachedExecutor accepts generic EvmFactory

The `CachedExecutor::new()` constructor SHALL accept a generic `EvmFactory` parameter rather than hardcoding `OpEvmFactory`. On the host (when tracing is needed), callers provide `TracingEvmFactory`. In the guest, callers provide `OpEvmFactory`. The `CachedExecutor` generic over `E: Executor` remains unchanged; the factory generics flow through `KonaExecutor<P, H, Evm>`.

#### Scenario: host uses TracingEvmFactory
- **WHEN** `CachedExecutor` is constructed with `KonaExecutor<P, H, TracingEvmFactory>` on the host
- **THEN** block execution captures per-transaction EvmState traces via the tracing factory

#### Scenario: guest uses OpEvmFactory
- **WHEN** `CachedExecutor` is constructed with `KonaExecutor<P, H, OpEvmFactory>` in the guest
- **THEN** block execution proceeds identically to the current behavior with zero tracing overhead

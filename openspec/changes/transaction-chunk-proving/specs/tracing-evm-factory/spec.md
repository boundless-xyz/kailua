## ADDED Requirements

### Requirement: TracingOpEvm wraps OpEvm and captures tx-body EvmState

The system SHALL provide a `TracingOpEvm<DB, I>` struct that wraps `OpEvm<DB, I, PrecompilesMap>` and implements the `Evm` trait. The `transact_raw()` method SHALL clone `ResultAndState.state` into a shared trace buffer before returning. The trace buffer represents only the ordered block transaction body used for chunk proving. Block-level prelude and epilogue state transitions are handled separately and SHALL NOT append extra entries to the chunk trace buffer. All other `Evm` trait methods SHALL delegate to the inner `OpEvm` without modification.

#### Scenario: transact_raw captures state on success
- **WHEN** `TracingOpEvm::transact_raw(tx)` is called and the inner EVM returns `Ok(ResultAndState { result, state })`
- **THEN** `state` is cloned and appended to the shared trace buffer, and the original `Ok(ResultAndState)` is returned unmodified

#### Scenario: transact_raw propagates errors without capturing
- **WHEN** `TracingOpEvm::transact_raw(tx)` is called and the inner EVM returns `Err(e)`
- **THEN** the error is returned unmodified and no entry is added to the trace buffer

#### Scenario: transact_system_call does not append tx-body traces
- **WHEN** `TracingOpEvm::transact_system_call(...)` is called for block-level prelude or epilogue work
- **THEN** the call is delegated to the inner `OpEvm`, and no entry is appended to the chunk trace buffer

#### Scenario: delegation methods are transparent
- **WHEN** any `Evm` trait method other than `transact_raw()` is called on `TracingOpEvm`
- **THEN** the call is delegated to the inner `OpEvm` and the result is returned unmodified

### Requirement: TracingOpEvm implements Deref and DerefMut to OpContext

`TracingOpEvm<DB, I>` SHALL implement `Deref<Target = OpContext<DB>>` and `DerefMut<Target = OpContext<DB>>` by delegating to the inner `OpEvm`'s `ctx()` and `ctx_mut()` public inherent methods. This is required because `OpBlockExecutor` accesses EVM fields (block env, cfg, database) through Deref, not through the `Evm` trait.

#### Scenario: Deref-based field access works
- **WHEN** `OpBlockExecutor` accesses `self.evm.block()`, `self.evm.cfg`, or `self.evm.db_mut()` via Deref on a `TracingOpEvm`
- **THEN** the access resolves to the inner `OpContext<DB>` fields identically to `OpEvm`

#### Scenario: mutable Deref-based access works
- **WHEN** `OpBlockExecutor` calls `self.evm.db_mut().commit(state)` or `self.evm.db_mut().set_state_clear_flag(flag)` via DerefMut
- **THEN** the mutation applies to the inner `OpContext<DB>` database

### Requirement: TracingEvmFactory wraps OpEvmFactory and injects TracingOpEvm

The system SHALL provide a `TracingEvmFactory` struct that implements `EvmFactory` with `type Evm<DB, I> = TracingOpEvm<DB, I>`. The factory SHALL hold a shared `Arc<Mutex<Vec<EvmState>>>` trace buffer that is passed to each `TracingOpEvm` it creates.

#### Scenario: create_evm produces TracingOpEvm
- **WHEN** `TracingEvmFactory::create_evm(db, env)` is called
- **THEN** it creates an `OpEvm` via the inner `OpEvmFactory::create_evm(db, env)` and wraps it in `TracingOpEvm` with the shared trace buffer

#### Scenario: create_evm_with_inspector produces TracingOpEvm
- **WHEN** `TracingEvmFactory::create_evm_with_inspector(db, env, inspector)` is called
- **THEN** it creates an `OpEvm` via the inner `OpEvmFactory::create_evm_with_inspector(db, env, inspector)` and wraps it in `TracingOpEvm` with the shared trace buffer

#### Scenario: trace buffer accumulates across tx-body transactions within a block
- **WHEN** a block with N transactions is executed using a `TracingEvmFactory`-produced EVM
- **THEN** the trace buffer contains exactly N entries, one per ordered block transaction, in execution order, and block-level prelude / epilogue work does not add extra entries

### Requirement: CachedExecutor accepts generic EvmFactory

The `CachedExecutor::new()` constructor SHALL accept a generic `EvmFactory` parameter rather than hardcoding `OpEvmFactory`. On the host (when tracing is needed), callers provide `TracingEvmFactory`. In the guest, callers provide `OpEvmFactory`. The `CachedExecutor` generic over `E: Executor` remains unchanged; the factory generics flow through `KonaExecutor<P, H, Evm>`.

#### Scenario: host uses TracingEvmFactory
- **WHEN** `CachedExecutor` is constructed with `KonaExecutor<P, H, TracingEvmFactory>` on the host
- **THEN** block execution captures per-transaction EvmState traces via the tracing factory

#### Scenario: guest uses OpEvmFactory
- **WHEN** `CachedExecutor` is constructed with `KonaExecutor<P, H, OpEvmFactory>` in the guest
- **THEN** block execution proceeds identically to the current behavior with zero tracing overhead

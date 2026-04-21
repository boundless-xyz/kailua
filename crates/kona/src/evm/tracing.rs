use alloy_evm::op_revm::{OpContext, OpHaltReason, OpSpecId, OpTransaction};
use alloy_evm::precompiles::PrecompilesMap;
use alloy_evm::revm::context::result::{EVMError, ResultAndState};
use alloy_evm::revm::context::{BlockEnv, TxEnv};
use alloy_evm::revm::inspector::NoOpInspector;
use alloy_evm::revm::Inspector;
use alloy_evm::{Database, Evm, EvmEnv, EvmFactory};
use alloy_op_evm::{OpEvm, OpEvmFactory, OpTxError};
use alloy_primitives::{Address, Bytes};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// EVM wrapper that captures `ResultAndState` on successful `transact_raw()`.
pub struct TracingEvm<E: Evm> {
    /// EVM used for execution
    pub inner: E,
    /// Result collection target
    pub traces: Arc<Mutex<Vec<ResultAndState<E::HaltReason>>>>,
}

impl<E: Evm> TracingEvm<E> {
    pub fn new(inner: E, traces: Arc<Mutex<Vec<ResultAndState<E::HaltReason>>>>) -> Self {
        Self { inner, traces }
    }
}

impl<E: Evm> Evm for TracingEvm<E> {
    type DB = E::DB;
    type Tx = E::Tx;
    type Error = E::Error;
    type HaltReason = E::HaltReason;
    type Spec = E::Spec;
    type BlockEnv = E::BlockEnv;
    type Precompiles = E::Precompiles;
    type Inspector = E::Inspector;

    fn block(&self) -> &Self::BlockEnv {
        self.inner.block()
    }

    fn chain_id(&self) -> u64 {
        self.inner.chain_id()
    }

    /// Executes a transaction, captures the full `ResultAndState` on success, then
    /// returns the original result unmodified.
    fn transact_raw(
        &mut self,
        tx: Self::Tx,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        let result = self.inner.transact_raw(tx)?;
        self.traces.lock().unwrap().push(result.clone());
        Ok(result)
    }

    /// System calls flow through without being appended to the tx-body trace buffer.
    fn transact_system_call(
        &mut self,
        caller: Address,
        contract: Address,
        data: Bytes,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        self.inner.transact_system_call(caller, contract, data)
    }

    fn finish(self) -> (Self::DB, EvmEnv<Self::Spec, Self::BlockEnv>)
    where
        Self: Sized,
    {
        self.inner.finish()
    }

    fn set_inspector_enabled(&mut self, enabled: bool) {
        self.inner.set_inspector_enabled(enabled)
    }

    fn components(&self) -> (&Self::DB, &Self::Inspector, &Self::Precompiles) {
        self.inner.components()
    }

    fn components_mut(&mut self) -> (&mut Self::DB, &mut Self::Inspector, &mut Self::Precompiles) {
        self.inner.components_mut()
    }
}

/// Factory that wraps `OpEvmFactory` and produces [`TracingEvm`]-wrapped `OpEvm`
/// instances sharing a single trace buffer. Drain with [`take_traces`](Self::take_traces)
/// at each per-block boundary; the shared buffer is an `Arc<Mutex<_>>` so clones of the
/// factory (e.g., via `KonaExecutor`) accumulate into the same trace vec.
///
/// # Per-block boundary is a caller convention (review finding L-1)
///
/// The shared buffer has no intrinsic notion of "block boundary" — callers MUST
/// invoke [`take_traces`](Self::take_traces) between blocks to prevent interleaving.
/// In the zkVM (single-threaded) this is guaranteed by construction. On the host, if
/// a caller ever runs two blocks concurrently against clones of the same factory
/// (`Arc<Mutex>` shares the buffer), traces will interleave silently — the
/// `traces.len() == block_txs.len()` sanity check in the chunk guest catches a length
/// mismatch but not correct-length interleaving. The chunk guest always creates a
/// fresh factory per chunk (`TracingOpEvmFactory::new()` in `run_core_client`), so
/// the host-concurrency concern is confined to host witness construction. Document
/// and enforce convention there.
#[derive(Clone, Debug)]
pub struct TracingOpEvmFactory {
    pub inner: OpEvmFactory,
    pub traces: Arc<Mutex<Vec<ResultAndState<OpHaltReason>>>>,
}

impl TracingOpEvmFactory {
    /// Create a factory with a fresh shared trace buffer.
    pub fn new() -> Self {
        Self {
            inner: OpEvmFactory::default(),
            traces: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Atomically drain and return the accumulated traces. Subsequent executions
    /// start with an empty trace buffer.
    pub fn take_traces(&self) -> Vec<ResultAndState<OpHaltReason>> {
        std::mem::take(&mut *self.traces.lock().unwrap())
    }
}

impl Default for TracingOpEvmFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl EvmFactory for TracingOpEvmFactory {
    type Evm<DB: Database, I: Inspector<OpContext<DB>>> = TracingEvm<OpEvm<DB, I, PrecompilesMap>>;
    type Context<DB: Database> = OpContext<DB>;
    type Tx = OpTransaction<TxEnv>;
    type Error<DBError: core::error::Error + Send + Sync + 'static> = EVMError<DBError, OpTxError>;
    type HaltReason = OpHaltReason;
    type Spec = OpSpecId;
    type BlockEnv = BlockEnv;
    type Precompiles = PrecompilesMap;

    fn create_evm<DB: Database>(
        &self,
        db: DB,
        input: EvmEnv<OpSpecId>,
    ) -> Self::Evm<DB, NoOpInspector> {
        TracingEvm::new(self.inner.create_evm(db, input), self.traces.clone())
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        input: EvmEnv<OpSpecId>,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        TracingEvm::new(
            self.inner.create_evm_with_inspector(db, input, inspector),
            self.traces.clone(),
        )
    }
}

/// Shared trace buffer keyed by L2 block number. Used by [`ChunkingEvmFactory`] and
/// [`run_core_client`](crate::client::core::run_core_client) to optionally capture
/// per-block `ResultAndState` traces during a monolithic pass so a subsequent run can
/// be replayed with `Chunk` entries built from the captured data. Alias exists to
/// keep the clippy `type_complexity` lint happy across call sites.
pub type ChunkTraceCollector = Arc<Mutex<HashMap<u64, Vec<ResultAndState<OpHaltReason>>>>>;

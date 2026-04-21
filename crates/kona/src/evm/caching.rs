//! Chunking EVM wrapper for injecting pre-computed per-transaction results.
//!
//! [`ChunkingEvm`] wraps any `E: Evm` and intercepts `transact_raw()` to return pre-computed
//! [`ResultAndState`] entries (captured by `TracingEvmFactory` on the host) for transactions
//! covered by the supplied [`PartialExecution`] set, instead of re-executing them.
//!
//! [`ChunkingEvmFactory`] wraps [`OpEvmFactory`] and dispenses [`ChunkingEvm`] instances
//! keyed by the target block number (`input.block_env.number`). This lets the aggregation
//! guest run blocks through the standard `CachedExecutor` → `StatelessL2Builder::build_block`
//! → `OpBlockExecutor` pipeline, substituting pre-computed chunk results for the tx-body
//! transactions while prelude and epilogue continue to execute normally. The resulting
//! `BlockBuildingOutcome` (state root, receipts root, gas used) is byte-exact with
//! monolithic execution.

use crate::evm::tracing::ChunkTraceCollector;
use crate::evm::PartialExecution;
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

/// EVM wrapper that serves pre-computed `ResultAndState` entries from a set of
/// [`PartialExecution`]s covering this block's ordered transaction body.
pub struct ChunkingEvm<E: Evm> {
    /// Actual EVM implementation
    pub inner: E,
    /// Remaining chunks for this block in *reverse* execution order — `last()` is the
    /// currently-active chunk, `pop()` discards it once exhausted.
    pub chunks: Vec<PartialExecution>,
    /// Cursor into `chunks.last().results`. Reset to 0 whenever a chunk is exhausted
    /// and popped off.
    pub cursor: usize,
    /// Block number this EVM is executing. Set at construction time from
    /// `input.block_env.number`; used as the key when appending captured
    /// `ResultAndState` into `block_traces` below (test/host trace capture).
    pub block_number: u64,
    /// Optional per-block trace collector. When `Some`, every successful
    /// `transact_raw()` pushes its `ResultAndState` into
    /// `block_traces.lock()[&block_number]` (Vec created on demand). This is the
    /// same capture semantic as `TracingEvm`, but folded directly into
    /// `ChunkingEvm` so a single factory can be configured for capture during a
    /// monolithic run (empty chunks map → every call delegates to inner OpEvm and
    /// is captured as ground truth) and for replay during a second run (chunks
    /// populated → replayed `ResultAndState` entries served from `chunk.results`).
    pub block_traces: Option<ChunkTraceCollector>,
}

impl<E: Evm> ChunkingEvm<E> {
    /// Wraps `inner` and prepares the chunk cursor, optionally attaching a per-block
    /// trace collector. `chunks` must be in ascending execution order (matching
    /// `tx_start`); this constructor reverses the vec so `pop()` yields the next chunk.
    ///
    /// When `block_traces` is `Some`, each successful `transact_raw()` appends its
    /// `ResultAndState` into `block_traces[block_number]`. Used by the integration test
    /// harness to capture ground-truth traces from a monolithic `run_core_client` run
    /// (empty chunks → `ChunkingEvm` delegates to inner `OpEvm`, results are captured
    /// exactly as `TracingEvm` would).
    pub fn new_with_traces(
        inner: E,
        mut chunks: Vec<PartialExecution>,
        block_number: u64,
        block_traces: Option<ChunkTraceCollector>,
    ) -> Self {
        chunks.reverse();
        Self {
            inner,
            chunks,
            cursor: 0,
            block_number,
            block_traces,
        }
    }
}

impl<E: Evm<HaltReason = OpHaltReason>> Evm for ChunkingEvm<E>
where
    E::DB: alloy_evm::revm::Database,
{
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

    /// Returns the next pre-computed `ResultAndState` if an available chunk still has
    /// results to serve; otherwise delegates to the inner EVM.
    ///
    /// # Binding of replayed results to the chunk proof
    ///
    /// Each returned `ResultAndState` is taken verbatim from `chunk.results`, but the
    /// chunk is authenticated by [`crate::client::stitching::stitch_chunks`] through
    /// the six-input `chunk_trace`:
    ///
    /// ```text
    /// chunk_trace = SHA256(tx_hash || pre_db_hash || post_db_hash
    ///                      || pre_evm_hash || post_evm_hash || results_hash)
    /// ```
    ///
    /// where `results_hash` is the canonical SHA256 of the per-tx `ResultAndState`
    /// trace (see [`crate::precondition::chunking::hash_results`]). The chunk guest
    /// captures its own trace via `TracingOpEvmFactory` during execution and folds
    /// the hash into its journal. The aggregation side recomputes `hash_results`
    /// over `chunk.results` and uses the same formula to reconstruct the expected
    /// chunk journal — so any tampering with `chunk.results` (reorder,
    /// substitution, forged value, variant change) produces a mismatching journal
    /// and `env::verify()` rejects the chunk proof's assumption. The replayed
    /// `ResultAndState` entries are therefore cryptographically bound to the
    /// authenticated chunk proof; `ChunkingEvm` is safe to use them without
    /// per-call verification.
    fn transact_raw(
        &mut self,
        tx: Self::Tx,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        // Drop the active chunk (`chunks.last()`) once its results are exhausted,
        // resetting the cursor for whatever chunk comes next.
        if let Some(chunk) = self.chunks.last() {
            if self.cursor >= chunk.results.len() {
                self.chunks.pop();
                self.cursor = 0;
            }
        }

        // Serve from the active chunk when possible.
        let result = if let Some(chunk) = self.chunks.last() {
            let out = chunk.results[self.cursor].clone();
            self.cursor += 1;

            // Pre-load every account referenced by the replayed state diff into
            // the database's cache. `State<TrieDB>::commit` panics if the
            // executor later tries to mutate an account that was never loaded
            // via a `basic()` / `storage()` call (revm's `apply_account_state`
            // requires the address to be present). In the monolithic path
            // these reads happen naturally as the EVM executes; here we skip
            // execution and return pre-computed state, so we must ensure every
            // touched account — and every touched storage slot — is cache-
            // resident before returning. This restores the invariant that the
            // executor's commit step can rely on without behaving differently
            // between the monolithic and chunked paths.
            //
            // NOTE: `Database` here is `alloy_evm::revm::Database` (re-exported
            // from revm), which is the trait whose `basic`/`storage` methods
            // State<TrieDB> implements. The outer `Database` imported at the top
            // of this file is `alloy_evm::Database`, a thinner wrapper.
            use alloy_evm::revm::Database as RevmDatabase;
            let db = self.inner.db_mut();
            for (addr, account) in out.state.iter() {
                let _ = RevmDatabase::basic(db, *addr);
                for slot in account.storage.keys() {
                    let _ = RevmDatabase::storage(db, *addr, *slot);
                }
            }

            Ok(out)
        } else {
            self.inner.transact_raw(tx)
        };

        // Optional capture: when a per-block trace collector was attached via
        // `new_with_traces`, append the ResultAndState keyed by this block's
        // number. The chunk replay path (served from chunk.results) also
        // captures — benign since the captured trace equals the authenticated
        // `chunk.results` and callers choose whether to consume the buffer.
        if let (Ok(r), Some(traces)) = (&result, &self.block_traces) {
            traces
                .lock()
                .unwrap()
                .entry(self.block_number)
                .or_default()
                .push(r.clone());
        }
        result
    }

    /// Delegates system calls to the inner EVM. Block-level prelude and epilogue work
    /// (beacon root, blockhash ring, Canyon deployer, post-block balance increments)
    /// runs through the inner OpEvm unchanged and does not consume chunk results.
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

/// Factory that wraps `OpEvmFactory` and dispenses [`ChunkingEvm`] instances seeded with
/// per-block chunk data keyed by `input.block_env.number`.
///
/// The chunk map is consumed destructively per block: `create_evm` removes the entry for
/// the target block so repeated calls for the same block would receive an empty chunk
/// vec (and therefore fall through to the inner EVM). In practice each block creates a
/// fresh `ChunkingEvm` via one `create_evm` call from `StatelessL2Builder::build_block`.
///
/// Blocks without chunk data (no entry in the map) get a `ChunkingEvm` with an empty
/// chunk vec, which is semantically identical to the plain `OpEvm` path — every
/// `transact_raw()` delegates to the inner.
#[derive(Clone, Debug)]
pub struct ChunkingEvmFactory {
    pub inner: OpEvmFactory,
    /// Per-block chunk data keyed by block number. Arc<Mutex<...>> because `EvmFactory`
    /// requires `Clone + Send + Sync + 'static` and the factory is cloned by the
    /// executor pipeline.
    pub chunks: Arc<Mutex<HashMap<u64, Vec<PartialExecution>>>>,
    /// Optional per-block `ResultAndState` trace collector threaded into every
    /// `ChunkingEvm` this factory produces. When `Some`, each `ChunkingEvm`
    /// appends successful `transact_raw()` results into
    /// `block_traces[block_number]`. Used by the integration test harness to
    /// capture ground-truth traces during a monolithic run (empty chunks) and
    /// then rebuild `Chunk` entries for a subsequent replay run.
    pub block_traces: Option<ChunkTraceCollector>,
}

impl ChunkingEvmFactory {
    /// Constructs a factory with the given block-keyed chunk data. The map is consumed
    /// destructively on each `create_evm` call.
    pub fn new(chunks: HashMap<u64, Vec<PartialExecution>>) -> Self {
        Self {
            inner: OpEvmFactory::default(),
            chunks: Arc::new(Mutex::new(chunks)),
            block_traces: None,
        }
    }

    /// Variant of [`new`](Self::new) that also attaches a per-block trace collector.
    /// Every `ChunkingEvm` produced by this factory will append successful
    /// `transact_raw()` results into `block_traces[block_number]`. Drain via
    /// `block_traces.lock()` at the per-block boundary the caller chooses (see
    /// the integration test in `crates/kona/src/client/core.rs`).
    pub fn new_with_traces(
        chunks: HashMap<u64, Vec<PartialExecution>>,
        block_traces: Option<ChunkTraceCollector>,
    ) -> Self {
        Self {
            inner: OpEvmFactory::default(),
            chunks: Arc::new(Mutex::new(chunks)),
            block_traces,
        }
    }

    /// Removes and returns the chunks for the given block number, or an empty vec.
    pub fn take_chunks(&self, block_number: u64) -> Vec<PartialExecution> {
        self.chunks
            .lock()
            .unwrap()
            .remove(&block_number)
            .unwrap_or_default()
    }
}

impl EvmFactory for ChunkingEvmFactory {
    type Evm<DB: Database, I: Inspector<OpContext<DB>>> = ChunkingEvm<OpEvm<DB, I, PrecompilesMap>>;
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
        let block_number = input.block_env.number.to::<u64>();
        let chunks = self.take_chunks(block_number);
        ChunkingEvm::new_with_traces(
            self.inner.create_evm(db, input),
            chunks,
            block_number,
            self.block_traces.clone(),
        )
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        input: EvmEnv<OpSpecId>,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        let block_number = input.block_env.number.to::<u64>();
        let chunks = self.take_chunks(block_number);
        ChunkingEvm::new_with_traces(
            self.inner.create_evm_with_inspector(db, input, inspector),
            chunks,
            block_number,
            self.block_traces.clone(),
        )
    }
}

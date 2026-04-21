//! Chunking EVM wrapper for injecting pre-computed per-transaction results.
//!
//! [`CachedEvm`] wraps any `E: Evm` and intercepts `transact_raw()` to return pre-computed
//! [`ResultAndState`] entries for transactions covered by the supplied
//! [`PartialExecution`] set, instead of re-executing them.
//!
//! The same type also serves as the *capture* wrapper: constructed with an empty chunk
//! map and a `Some(block_traces)` collector via
//! [`CachedEvmFactory::new_with_traces`], every `transact_raw()` delegates to the inner
//! `OpEvm` and its `ResultAndState` is appended to `block_traces[block_number]`. The
//! chunk guest uses this mode to fold per-tx results into `chunk_trace` via
//! `hash_results(&traces)`.
//!
//! [`CachedEvmFactory`] wraps [`OpEvmFactory`] and dispenses [`CachedEvm`] instances
//! keyed by the target block number (`input.block_env.number`). This lets the aggregation
//! guest run blocks through the standard `CachedExecutor` → `StatelessL2Builder::build_block`
//! → `OpBlockExecutor` pipeline, substituting pre-computed chunk results for the tx-body
//! transactions while prelude and epilogue continue to execute normally. The resulting
//! `BlockBuildingOutcome` (state root, receipts root, gas used) is byte-exact with
//! monolithic execution.

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
use std::sync::{Arc, Mutex};

/// Shared trace buffer, one inner `Vec` per EVM instance created by
/// [`CachedEvmFactory`] in ascending creation order (which, during normal
/// derivation, matches ascending L2 block order). Each `CachedEvm` appends its
/// per-tx `ResultAndState` entries to the last inner `Vec` via
/// `last_mut().push(...)`; the factory pushes a fresh empty inner `Vec` on every
/// `create_evm` / `create_evm_with_inspector` call.
pub type TransactionResultCollector = Arc<Mutex<Vec<Vec<ResultAndState<OpHaltReason>>>>>;

/// EVM wrapper that serves pre-computed `ResultAndState` entries from a set of
/// [`PartialExecution`]s covering this block's ordered transaction body.
pub struct CachedEvm<E: Evm> {
    /// Remaining chunks for this block in *reverse* execution order
    pub cache: Vec<PartialExecution>,
    /// Actual EVM implementation
    pub evm: E,
    /// Optional trace collector shared across all `CachedEvm` instances produced
    /// by one [`CachedEvmFactory`]. When `Some`, every successful
    /// `transact_raw()` pushes its `ResultAndState` into the **last** inner `Vec`
    /// of the shared buffer — the factory pushes a fresh empty slot on every
    /// `create_evm` call, so `last_mut()` always targets "this EVM's slot". This
    /// lets one factory type serve both passes: capture during a monolithic run
    /// (empty chunks map → every call delegates to inner `OpEvm` and is captured
    /// as ground truth) and replay during a second run (chunks populated →
    /// replayed `ResultAndState` entries served from `chunk.results`).
    pub collection_target: Option<TransactionResultCollector>,
}

impl<E: Evm> CachedEvm<E> {
    /// Wraps `inner` and prepares the chunk cache, optionally attaching a shared
    /// trace collector. `chunks` must be in ascending execution order (matching
    /// `tx_start`); this constructor reverses the outer vec so `pop()` yields the
    /// next chunk, and each chunk's `results` vec so `results.pop()` yields the
    /// next pre-computed `ResultAndState` within that chunk.
    ///
    /// When `block_traces` is `Some`, each successful `transact_raw()` appends its
    /// `ResultAndState` into the last inner `Vec` of `block_traces`. Used both by
    /// the chunk guest (single-block capture for `chunk_trace`) and by the
    /// integration test harness (capture ground-truth traces from a monolithic
    /// `run_core_client` run with empty chunks; `CachedEvm` then delegates every
    /// call to the inner `OpEvm`). The caller is responsible for ensuring the
    /// last slot of `block_traces` is the one this EVM should append to —
    /// `CachedEvmFactory` handles this by pushing a fresh empty slot on each
    /// `create_evm` call before constructing the `CachedEvm`.
    pub fn new_with_traces(
        inner: E,
        mut chunks: Vec<PartialExecution>,
        block_traces: Option<TransactionResultCollector>,
    ) -> Self {
        chunks.reverse();
        for chunk in &mut chunks {
            chunk.results.reverse();
        }
        Self {
            evm: inner,
            cache: chunks,
            collection_target: block_traces,
        }
    }
}

impl<E: Evm<HaltReason = OpHaltReason>> Evm for CachedEvm<E>
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
        self.evm.block()
    }

    fn chain_id(&self) -> u64 {
        self.evm.chain_id()
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
    /// captures its own trace via [`CachedEvmFactory`] in capture-only mode (empty
    /// chunk map + `Some(collector)`) during execution and folds the hash into its
    /// journal. The aggregation side recomputes `hash_results`
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
        // Peel off any exhausted chunks (empty `results`) from the cache so that
        // `last_mut()` below either yields an active chunk with work left, or `None`.
        while self.cache.last().is_some_and(|c| c.results.is_empty()) {
            self.cache.pop();
        }

        // Serve from the active chunk when possible. `results.pop()` yields the
        // next pre-computed entry (chunks were stored reversed at construction).
        let result = if let Some(chunk) = self.cache.last_mut() {
            let out = chunk.results.pop().expect("peel loop ensures non-empty");

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
            let db = self.evm.db_mut();
            for (addr, account) in out.state.iter() {
                let _ = RevmDatabase::basic(db, *addr);
                for slot in account.storage.keys() {
                    let _ = RevmDatabase::storage(db, *addr, *slot);
                }
            }

            Ok(out)
        } else {
            self.evm.transact_raw(tx)
        };

        // Optional capture: when a shared trace collector was attached via
        // `new_with_traces`, append the ResultAndState to the *last* inner `Vec`
        // of the shared buffer. `CachedEvmFactory` pushes a fresh empty slot on
        // each `create_evm` call, so `last_mut()` always targets "this EVM's
        // slot". The chunk replay path (served from chunk.results) also captures
        // — benign since the captured trace equals the authenticated
        // `chunk.results` and callers choose whether to consume the buffer.
        if let (Ok(r), Some(traces)) = (&result, &self.collection_target) {
            let mut guard = traces.lock().unwrap();
            guard
                .last_mut()
                .expect(
                    "CachedEvmFactory pushes an empty slot before constructing \
                     a CachedEvm; last_mut() must exist",
                )
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
        self.evm.transact_system_call(caller, contract, data)
    }

    fn finish(self) -> (Self::DB, EvmEnv<Self::Spec, Self::BlockEnv>)
    where
        Self: Sized,
    {
        self.evm.finish()
    }

    fn set_inspector_enabled(&mut self, enabled: bool) {
        self.evm.set_inspector_enabled(enabled)
    }

    fn components(&self) -> (&Self::DB, &Self::Inspector, &Self::Precompiles) {
        self.evm.components()
    }

    fn components_mut(&mut self) -> (&mut Self::DB, &mut Self::Inspector, &mut Self::Precompiles) {
        self.evm.components_mut()
    }
}

/// Factory that wraps `OpEvmFactory` and dispenses [`CachedEvm`] instances seeded with
/// positional per-block chunk data.
///
/// The `cache` field holds an outer `Vec<Vec<PartialExecution>>` in **reverse** execution
/// order so that `create_evm` can `pop()` the next block's chunks off the end in O(1)
/// and move them into the new [`CachedEvm`]. During normal derivation `create_evm` is
/// called exactly once per L2 block in ascending block order, so positional index
/// (input-order, pre-reversal) maps 1:1 to block position.
///
/// Blocks without chunk data are represented by an empty inner `Vec`, which produces a
/// `CachedEvm` with an empty cache — every `transact_raw()` then delegates to the inner
/// `OpEvm`, semantically identical to the plain `OpEvm` path.
///
/// If `create_evm` is called more times than there are chunk entries (or called after
/// the cache is drained), `take_next_chunks()` returns an empty `Vec`, and the
/// resulting EVM falls through to the inner `OpEvm`.
#[derive(Clone, Debug)]
pub struct CachedEvmFactory {
    /// The factory used to instantiate the underlying EVM instances
    pub inner: OpEvmFactory,
    /// Per-block chunk data stored in **reverse** execution order.
    pub cache: Arc<Mutex<Vec<Vec<PartialExecution>>>>,
    /// Optional per-transaction `ResultAndState` collector
    pub block_traces: Option<TransactionResultCollector>,
}

impl CachedEvmFactory {
    /// Constructs a factory with the given positional per-block chunk data. The
    /// input is expected in **execution order** (outer index 0 = first block
    /// served); the constructor reverses it internally so that `create_evm`
    /// pops from the end in execution order.
    pub fn new(cache: Vec<Vec<PartialExecution>>) -> Self {
        Self::new_with_traces(cache, None)
    }

    /// Variant of [`new`](Self::new) that also attaches a shared trace collector.
    /// Every `CachedEvm` produced by this factory will append successful
    /// `transact_raw()` results into the last inner `Vec` of the collector (the
    /// factory pushes a fresh slot on each `create_evm` call). Drain via
    /// [`take_all_block_traces`](Self::take_all_block_traces) or by
    /// `std::mem::take` on the shared buffer at the per-block boundary the
    /// caller chooses (see the integration test in `crates/kona/src/client/core.rs`).
    pub fn new_with_traces(
        mut cache: Vec<Vec<PartialExecution>>,
        block_traces: Option<TransactionResultCollector>,
    ) -> Self {
        cache.reverse();
        Self {
            inner: OpEvmFactory::default(),
            cache: Arc::new(Mutex::new(cache)),
            block_traces,
        }
    }

    /// Pops and returns the next block's chunks from the cache, or an empty vec
    /// when the cache is exhausted. The outer `Vec` was reversed at construction
    /// time so `pop()` yields blocks in execution order.
    pub fn take_next_chunks(&self) -> Vec<PartialExecution> {
        self.cache.lock().unwrap().pop().unwrap_or_default()
    }

    /// Atomically drains and returns the shared trace buffer — one inner `Vec` per
    /// EVM instance this factory produced, in creation order. Returns an empty vec
    /// when no trace collector is attached. During normal derivation one EVM is
    /// created per L2 block in ascending block order, so the returned outer index
    /// maps 1:1 to block position starting from the first block the factory served.
    pub fn take_all_block_traces(&self) -> Vec<Vec<ResultAndState<OpHaltReason>>> {
        self.block_traces
            .as_ref()
            .map(|t| std::mem::take(&mut *t.lock().unwrap()))
            .unwrap_or_default()
    }

    /// Push an empty slot onto the shared trace buffer so that the next
    /// `CachedEvm`'s `transact_raw` captures land in a fresh per-EVM `Vec`. No-op
    /// when no collector is attached. Called from both `create_evm` and
    /// `create_evm_with_inspector`.
    fn push_trace_slot(&self) {
        if let Some(traces) = &self.block_traces {
            traces.lock().unwrap().push(Vec::new());
        }
    }
}

impl EvmFactory for CachedEvmFactory {
    type Evm<DB: Database, I: Inspector<OpContext<DB>>> = CachedEvm<OpEvm<DB, I, PrecompilesMap>>;
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
        let chunks = self.take_next_chunks();
        self.push_trace_slot();
        CachedEvm::new_with_traces(
            self.inner.create_evm(db, input),
            chunks,
            self.block_traces.clone(),
        )
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        input: EvmEnv<OpSpecId>,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        let chunks = self.take_next_chunks();
        self.push_trace_slot();
        CachedEvm::new_with_traces(
            self.inner.create_evm_with_inspector(db, input, inspector),
            chunks,
            self.block_traces.clone(),
        )
    }
}

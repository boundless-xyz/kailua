//! Cached EVM wrapper for capturing and injecting pre-computed per-transaction results.

use crate::evm::PartialExecution;
use alloy_evm::op_revm::{OpContext, OpHaltReason, OpSpecId, OpTransaction};
use alloy_evm::precompiles::PrecompilesMap;
use alloy_evm::revm::context::result::{EVMError, ResultAndState};
use alloy_evm::revm::context::{BlockEnv, TxEnv};
use alloy_evm::revm::inspector::NoOpInspector;
use alloy_evm::revm::state::AccountStatus;
use alloy_evm::revm::Database as RevmDatabase;
use alloy_evm::revm::Inspector;
use alloy_evm::{Database, Evm, EvmEnv, EvmFactory};
use alloy_op_evm::{OpEvm, OpEvmFactory, OpTxError};
use alloy_primitives::{keccak256, Address, Bytes, B256};
use std::sync::{Arc, Mutex};

/// Shared trace buffer, one inner `Vec` per EVM instance created by
/// [`CachedEvmFactory`] in ascending creation order (which, during normal
/// derivation, matches ascending L2 block order). Each inner entry pairs the
/// per-tx identity hash (`keccak256(tx.enveloped_tx)` — the EIP-2718 tx hash)
/// with its captured `ResultAndState`. Pre-transaction `AccountInfo` is *not*
/// stored alongside: `ResultAndState.state[addr].original_info` already carries
/// the chunk's first-load view of each touched address, and `AccountRkyv`
/// preserves it through the rkyv round-trip, so the chunk proof's
/// `results_hash` authenticates it directly.
pub type TransactionResultCollector = Arc<Mutex<Vec<Vec<(B256, ResultAndState<OpHaltReason>)>>>>;

/// EVM wrapper that serves pre-computed `ResultAndState` entries from a set of
/// [`PartialExecution`]s covering this block's ordered transaction body.
pub struct CachedEvm<E: Evm> {
    /// Remaining chunks for this block in *reverse* execution order
    pub cache: Vec<PartialExecution>,
    /// Actual EVM implementation
    pub evm: E,
    /// Optional trace collector shared across all `CachedEvm` instances produced
    /// by one [`CachedEvmFactory`].
    pub collection_target: Option<TransactionResultCollector>,
}

impl<E: Evm> CachedEvm<E> {
    /// Wraps `inner` and prepares the chunk cache, optionally attaching a shared
    /// trace collector. `chunks` must be in ascending execution order (matching
    /// `tx_start`); this constructor reverses the outer vec so `pop()` yields the
    /// next chunk, and each chunk's `results`/`tx_hashes` vecs so `results.pop()`
    /// (and the parallel `tx_hashes.pop()`) yield the next pre-computed entry in
    /// execution order.
    pub fn new_with_traces(
        evm: E,
        mut cache: Vec<PartialExecution>,
        collection_target: Option<TransactionResultCollector>,
    ) -> Self {
        cache.reverse();
        for chunk in &mut cache {
            chunk.results.reverse();
            chunk.tx_hashes.reverse();
        }
        Self {
            evm,
            cache,
            collection_target,
        }
    }
}

impl<E: Evm<HaltReason = OpHaltReason, Tx = OpTransaction<TxEnv>>> Evm for CachedEvm<E>
where
    E::DB: alloy_evm::revm::Database,
    BlockEnv: PartialEq<<E as Evm>::BlockEnv>
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

    /// Returns the next pre-computed `ResultAndState` if the top of the cache has an
    /// entry whose `tx_hash` matches the incoming tx's EIP-2718 hash; otherwise
    /// delegates to the inner EVM without consuming a cache entry.
    fn transact_raw(
        &mut self,
        tx: Self::Tx,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        // Compute the incoming tx's identity hash
        let incoming_hash = tx
            .enveloped_tx
            .as_ref()
            .map(|b| keccak256(b.as_ref()))
            .expect("CachedEvm::transact_raw: OpTransaction.enveloped_tx must be populated");

        // Peel off any exhausted chunks
        while self.cache.last().is_some_and(|c| c.results.is_empty()) {
            self.cache.pop();
        }

        // Serve from the active chunk only if tx hashes match
        let serve_cached = self
            .cache
            .last()
            .and_then(|c| c.tx_hashes.last())
            .is_some_and(|expected| *expected == incoming_hash);

        let result = if serve_cached {
            let chunk = self
                .cache
                .last_mut()
                .expect("serve_cached implies cache is non-empty");
            let _consumed_hash = chunk
                .tx_hashes
                .pop()
                .expect("serve_cached implies tx_hashes is non-empty");
            let res_state = chunk
                .results
                .pop()
                .expect("serve_cached implies results is non-empty");

            // Ensure this chunk aligns with the current block
            assert_eq!(&chunk.block_env, self.evm.block(), "BlockEnv mismatch");

            // Prestate authentication. For every address the chunk's tx touched:
            //   (a) per-slot: inner_db.storage_ref(addr, slot) must equal
            //       `EvmStorageSlot.original_value` (the value revm first read
            //       for that slot during the chunk's execution).
            //   (b) per-account: inner_db.basic_ref(addr) must equal
            //       `*account.original_info` (the `AccountInfo` revm first loaded
            //       for that address during the chunk's execution). `original_info`
            //       is now preserved across the rkyv round-trip by `AccountRkyv`,
            //       and folded into `results_hash` by `write_account`, so the
            //       chunk proof authenticates it.
            let db = self.evm.db_mut();
            for (addr, account) in res_state.state.iter() {
                // Always call `db.basic(addr)` to warm State.cache for this address
                let actual_info = RevmDatabase::basic(db, *addr).map_err(|_| ()).expect(
                    "CachedEvm::transact_raw: inner DB basic read failed during \
                     prestate authentication",
                );
                if let Some(stored_info) = actual_info.as_ref() {
                    let expected_info = account.original_info.as_ref();
                    assert_eq!(
                        stored_info, expected_info,
                        "CachedEvm::transact_raw: account prestate mismatch at addr={addr}: \
                         chunk.original_info={expected_info:?} live_db={stored_info:?}"
                    );
                } else {
                    assert!(
                        account.status.contains(AccountStatus::Created)
                            || account.status.contains(AccountStatus::LoadedAsNotExisting),
                        "Unexpected AccountStatus for non-existing account"
                    );
                }

                // Per-slot prestate authentication. The `db.storage` call also
                // warms State.cache for this slot (needed for the subsequent
                // `db.commit` by OpBlockExecutor to find the account).
                for (slot, evm_slot) in account.storage.iter() {
                    let actual = RevmDatabase::storage(db, *addr, *slot)
                        .map_err(|_| ())
                        .expect(
                            "CachedEvm::transact_raw: inner DB storage read failed during \
                         prestate authentication",
                        );
                    assert_eq!(
                        actual, evm_slot.original_value,
                        "CachedEvm::transact_raw: storage prestate mismatch at \
                         addr={addr} slot={slot}: chunk.original_value={} live_db={}",
                        evm_slot.original_value, actual
                    );
                }
            }

            Ok(res_state)
        } else {
            // Execute transaction manually
            self.evm.transact_raw(tx)
        };

        // Capture results
        if let (Ok(r), Some(traces)) = (&result, &self.collection_target) {
            let mut guard = traces.lock().unwrap();
            guard
                .last_mut()
                .expect(
                    "CachedEvmFactory pushes an empty slot before constructing \
                     a CachedEvm; last_mut() must exist",
                )
                .push((incoming_hash, r.clone()));
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
    pub fn take_all_block_traces(&self) -> Vec<Vec<(B256, ResultAndState<OpHaltReason>)>> {
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

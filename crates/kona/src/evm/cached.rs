// Copyright 2026 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::client::log;
use crate::evm::expected;
use crate::evm::partial::PartialExecutionTrace;
use crate::evm::partial::{
    ActivePartialExecution, PartialExecution, PartialResultAndState, TransactionResultCollector,
};
use alloy_evm::op_revm::{OpContext, OpHaltReason, OpSpecId, OpTransaction};
use alloy_evm::precompiles::PrecompilesMap;
use alloy_evm::revm::context::result::{EVMError, ResultAndState};
use alloy_evm::revm::context::{BlockEnv, TxEnv};
use alloy_evm::revm::inspector::NoOpInspector;
use alloy_evm::revm::state::AccountStatus;
use alloy_evm::revm::{Database as RevmDatabase, Inspector};
use alloy_evm::{Database, Evm, EvmEnv, EvmFactory};
use alloy_op_evm::{OpEvm, OpEvmFactory, OpTxError};
use alloy_primitives::{keccak256, Address, Bytes};
use std::sync::{Arc, Mutex};

/// EVM wrapper that serves pre-computed `ResultAndState` entries
pub struct CachedEvm<E: Evm> {
    /// Remaining chunks for this block in *reverse* execution order
    cache: Vec<ActivePartialExecution>,
    /// Actual EVM implementation
    pub evm: E,
    /// OP hardfork spec for this EVM environment.
    spec_id: OpSpecId,
    /// Optional trace collector shared across all `CachedEvm` instances produced
    /// by one [`CachedEvmFactory`].
    pub collection_target: Option<TransactionResultCollector>,
}

impl<E: Evm> CachedEvm<E> {
    /// Wraps `inner` and prepares the chunk cache
    pub fn new_with_traces(
        evm: E,
        spec_id: OpSpecId,
        mut cache: Vec<PartialExecution>,
        collection_target: Option<TransactionResultCollector>,
    ) -> Self {
        cache.reverse();
        let cache = cache
            .into_iter()
            .map(|mut partial| {
                partial.results.reverse();
                partial.tx_hashes.reverse();
                ActivePartialExecution {
                    partial,
                    expected_state_verified: false,
                }
            })
            .collect();
        Self {
            evm,
            cache,
            spec_id,
            collection_target,
        }
    }
}

impl<E: Evm<HaltReason = OpHaltReason, Tx = OpTransaction<TxEnv>>> Evm for CachedEvm<E>
where
    E::DB: alloy_evm::revm::Database,
    BlockEnv: PartialEq<<E as Evm>::BlockEnv>,
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

    /// Returns the next pre-computed `ResultAndState` if the top entry matches
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
        while self
            .cache
            .last()
            .is_some_and(|c| c.partial.results.is_empty())
        {
            self.cache.pop();
        }

        // Serve from the active chunk only if tx hashes match
        let serve_cached = self
            .cache
            .last()
            .and_then(|c| c.partial.tx_hashes.last())
            .is_some_and(|expected| *expected == incoming_hash);

        let result = if serve_cached {
            log(&format!("CACHED {incoming_hash}"));
            let chunk = self
                .cache
                .last_mut()
                .expect("serve_cached implies cache is non-empty");
            let should_verify_expected_state = !chunk.expected_state_verified;
            let _consumed_hash = chunk
                .partial
                .tx_hashes
                .pop()
                .expect("serve_cached implies tx_hashes is non-empty");
            let res_state = chunk
                .partial
                .results
                .pop()
                .expect("serve_cached implies results is non-empty");

            // Ensure this chunk aligns with the current block
            assert_eq!(
                &chunk.partial.block_env,
                self.evm.block(),
                "BlockEnv mismatch"
            );

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
            if should_verify_expected_state {
                expected::verify_expected_state(db, &chunk.partial.expected_state, self.spec_id);
                chunk.expected_state_verified = true;
            }
            for entry in res_state.state.iter() {
                // Always call `db.basic(addr)` to warm State.cache for this address
                let actual_info = RevmDatabase::basic(db, entry.address)
                    .map_err(|_| ())
                    .expect(
                        "CachedEvm::transact_raw: inner DB basic read failed during \
                     prestate authentication",
                    );
                if let Some(stored_info) = actual_info.as_ref() {
                    let expected_info = &entry.account.original_info;
                    assert_eq!(
                        stored_info, expected_info,
                        "CachedEvm::transact_raw: account prestate mismatch at addr={}: \
                         chunk.original_info={expected_info:?} live_db={stored_info:?}",
                        entry.address
                    );
                } else {
                    assert!(
                        entry.account.status.contains(AccountStatus::Created)
                            || entry
                                .account
                                .status
                                .contains(AccountStatus::LoadedAsNotExisting),
                        "Unexpected AccountStatus for non-existing account"
                    );
                }

                // Per-slot prestate authentication. The `db.storage` call also
                // warms State.cache for this slot (needed for the subsequent
                // `db.commit` by OpBlockExecutor to find the account).
                for slot_entry in entry.account.storage.iter() {
                    let actual = RevmDatabase::storage(db, entry.address, slot_entry.slot)
                        .map_err(|_| ())
                        .expect(
                            "CachedEvm::transact_raw: inner DB storage read failed during \
                         prestate authentication",
                        );
                    assert_eq!(
                        actual,
                        slot_entry.slot_value.original_value,
                        "CachedEvm::transact_raw: storage prestate mismatch at \
                         addr={} slot={}: chunk.original_value={} live_db={}",
                        entry.address,
                        slot_entry.slot,
                        slot_entry.slot_value.original_value,
                        actual
                    );
                }
            }

            // Materialize the revm `ResultAndState` for the trait return.
            // `Into` is a single move (no element clones) — see
            // `From<PartialResultAndState> for ResultAndState`.
            Ok(res_state.into())
        } else {
            // Execute transaction manually
            let expected_state = self.collection_target.as_ref().map(|_| {
                expected::capture_required_expected_state(self.evm.db_mut(), self.spec_id)
            });
            let result = self.evm.transact_raw(tx);

            // Capture result. We clone the revm result for the trait return,
            // and drain the clone into a `PartialResultAndState` (sorted Vec
            // form) for the trace buffer.
            if let (Ok(r), Some(traces), Some(expected_state)) =
                (&result, &self.collection_target, expected_state)
            {
                let mut guard = traces.lock().unwrap();
                guard
                    .last_mut()
                    .expect(
                        "CachedEvmFactory pushes an empty slot before constructing \
                     a CachedEvm; last_mut() must exist",
                    )
                    .push(PartialExecutionTrace {
                        tx_hash: incoming_hash,
                        result: PartialResultAndState::from(r.clone()),
                        expected_state,
                    });
            }

            result
        };
        result
    }

    /// Delegates system calls to the inner EVM.
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
    /// Constructs a factory with the given positional per-block chunk data.
    pub fn new(cache: Vec<Vec<PartialExecution>>) -> Self {
        Self::new_with_traces(cache, None)
    }

    /// Variant of [`new`](Self::new) that also attaches a shared trace collector.
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

    /// Pops and returns the next block's chunks from the cache
    pub fn take_next_chunks(&self) -> Vec<PartialExecution> {
        self.cache.lock().unwrap().pop().unwrap_or_default()
    }

    /// Atomically drains and returns the shared trace buffer
    pub fn take_all_block_traces(&self) -> Vec<Vec<PartialExecutionTrace>> {
        self.block_traces
            .as_ref()
            .map(|t| std::mem::take(&mut *t.lock().unwrap()))
            .unwrap_or_default()
    }

    /// Push an empty slot onto the shared trace buffer
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
        let spec_id = input.cfg_env.spec;
        CachedEvm::new_with_traces(
            self.inner.create_evm(db, input),
            spec_id,
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
        let spec_id = input.cfg_env.spec;
        CachedEvm::new_with_traces(
            self.inner.create_evm_with_inspector(db, input, inspector),
            spec_id,
            chunks,
            self.block_traces.clone(),
        )
    }
}

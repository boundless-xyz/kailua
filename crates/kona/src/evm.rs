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
use crate::executor::Execution;
use crate::precondition::evm::{compute_pe_trace, hash_block_ctx, hash_results};
use crate::rkyv::evm::{
    AccountInfoRkyv, AccountStatusRkyv, BlockEnvRkyv, CacheStateRkyv, EvmStorageSlotRkyv,
    ExecutionResultRkyv, OpBlockExecutionCtxRkyv,
};
use crate::rkyv::primitives::{AddressDef, B256Def, U256Def};
use alloy_evm::op_revm::{OpContext, OpHaltReason, OpSpecId, OpTransaction};
use alloy_evm::precompiles::PrecompilesMap;
use alloy_evm::revm::context::result::{EVMError, ExecutionResult, ResultAndState};
use alloy_evm::revm::context::{BlockEnv, TxEnv};
use alloy_evm::revm::database::states::CacheAccount;
use alloy_evm::revm::database::CacheState;
use alloy_evm::revm::inspector::NoOpInspector;
use alloy_evm::revm::state::{Account, AccountInfo, AccountStatus, EvmStorageSlot};
use alloy_evm::revm::{Database as RevmDatabase, Inspector};
use alloy_evm::{Database, Evm, EvmEnv, EvmFactory};
use alloy_op_evm::{OpBlockExecutionCtx, OpEvm, OpEvmFactory, OpTxError};
use alloy_primitives::{keccak256, Address, Bytes, B256, U256};
use kona_proof::BootInfo;
use std::fmt::Debug;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct PartialStorageEntry {
    #[rkyv(with = U256Def)]
    pub slot: U256,
    #[rkyv(with = EvmStorageSlotRkyv)]
    pub slot_value: EvmStorageSlot,
}

#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct PartialAccount {
    #[rkyv(with = AccountInfoRkyv)]
    pub info: AccountInfo,
    #[rkyv(with = AccountInfoRkyv)]
    pub original_info: AccountInfo,
    pub transaction_id: u64,
    /// Sorted by slot key.
    pub storage: Vec<PartialStorageEntry>,
    #[rkyv(with = AccountStatusRkyv)]
    pub status: AccountStatus,
}

impl From<Account> for PartialAccount {
    fn from(value: Account) -> Self {
        let mut storage: Vec<PartialStorageEntry> = value
            .storage
            .into_iter()
            .map(|(slot, slot_value)| PartialStorageEntry { slot, slot_value })
            .collect();
        storage.sort_by_key(|e| e.slot);
        Self {
            info: value.info,
            original_info: *value.original_info,
            transaction_id: value.transaction_id as u64,
            storage,
            status: value.status,
        }
    }
}

impl From<PartialAccount> for Account {
    fn from(value: PartialAccount) -> Self {
        Account {
            info: value.info,
            original_info: Box::new(value.original_info),
            transaction_id: value.transaction_id as usize,
            storage: value
                .storage
                .into_iter()
                .map(|e| (e.slot, e.slot_value))
                .collect(),
            status: value.status,
        }
    }
}

#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct PartialStateEntry {
    #[rkyv(with = AddressDef)]
    pub address: Address,
    pub account: PartialAccount,
}

#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct PartialResultAndState {
    #[rkyv(with = ExecutionResultRkyv)]
    pub result: ExecutionResult<OpHaltReason>,
    /// Sorted by address.
    pub state: Vec<PartialStateEntry>,
}

impl From<ResultAndState<OpHaltReason>> for PartialResultAndState {
    fn from(r: ResultAndState<OpHaltReason>) -> Self {
        let mut state: Vec<PartialStateEntry> = r
            .state
            .into_iter()
            .map(|(address, acc)| PartialStateEntry {
                address,
                account: PartialAccount::from(acc),
            })
            .collect();
        state.sort_by_key(|e| e.address);
        Self {
            result: r.result,
            state,
        }
    }
}

impl From<PartialResultAndState> for ResultAndState<OpHaltReason> {
    fn from(value: PartialResultAndState) -> Self {
        Self {
            result: value.result,
            state: value
                .state
                .into_iter()
                .map(|e| (e.address, e.account.into()))
                .collect(),
        }
    }
}

/// Represents a proven transaction subsequence within a block.
#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct PartialExecution {
    /// The EIP-2718 tx hash for each entry in `results`.
    #[rkyv(with = rkyv::with::Map<B256Def>)]
    pub tx_hashes: Vec<B256>,
    /// Full per-tx execution results (ExecutionResult + sorted state)
    pub results: Vec<PartialResultAndState>,
    /// Block execution `BlockEnv` under which this chunk's transactions executed
    #[rkyv(with = BlockEnvRkyv)]
    pub block_env: BlockEnv,
    /// OP block execution context
    #[rkyv(with = OpBlockExecutionCtxRkyv)]
    pub op_block_ctx: OpBlockExecutionCtx,
}

impl PartialExecution {
    pub fn precondition_hash(&self) -> B256 {
        compute_pe_trace(
            hash_results(&self.tx_hashes, &self.results),
            hash_block_ctx(&self.block_env, &self.op_block_ctx),
        )
    }

    pub fn boot_info(&self, boot: &BootInfo) -> BootInfo {
        BootInfo {
            l1_head: B256::repeat_byte(0xFF),
            agreed_l2_output_root: self.op_block_ctx.parent_hash,
            claimed_l2_output_root: self.op_block_ctx.parent_hash,
            claimed_l2_block_number: self.block_env.number.to::<u64>().saturating_sub(1),
            chain_id: boot.chain_id,
            rollup_config: boot.rollup_config.clone(),
            l1_config: boot.l1_config.clone(),
        }
    }
}

pub fn cache_results(results: Vec<PartialResultAndState>) -> CacheState {
    let mut cache_state = CacheState::default();
    for result in results {
        for entry in result.state.into_iter() {
            let PartialStateEntry { address, account } = entry;
            let cache_account = cache_state.accounts.entry(address).or_insert_with(|| {
                if account.status.contains(AccountStatus::LoadedAsNotExisting) {
                    CacheAccount::new_loaded_not_existing()
                } else {
                    CacheAccount::new_loaded(account.original_info.clone(), Default::default())
                }
            });
            if let Some(plain) = cache_account.account.as_mut() {
                for slot_entry in account.storage.into_iter() {
                    plain
                        .storage
                        .entry(slot_entry.slot)
                        .or_insert(slot_entry.slot_value.original_value);
                }
                if let Some(code) = plain.info.code.take() {
                    cache_state
                        .contracts
                        .entry(plain.info.code_hash)
                        .or_insert(code);
                }
            }
        }
    }
    cache_state
}

/// Witness data for proving a single transaction subsequence within a block.
#[derive(Clone, Debug, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct PartialExecutionWitness {
    /// List of transactions to execute
    pub transactions: Vec<Vec<u8>>,
    /// Pre-state cache
    #[rkyv(with = CacheStateRkyv)]
    pub cache: CacheState,
    /// Block execution context
    #[rkyv(with = BlockEnvRkyv)]
    pub block_env: BlockEnv,
    /// OP Block context
    #[rkyv(with = OpBlockExecutionCtxRkyv)]
    pub op_block_ctx: OpBlockExecutionCtx,
}

impl PartialExecutionWitness {
    pub fn new(partial_execution: PartialExecution, transactions: Vec<Vec<u8>>) -> Self {
        let PartialExecution {
            results,
            block_env,
            op_block_ctx,
            ..
        } = partial_execution;

        PartialExecutionWitness {
            transactions,
            cache: cache_results(results),
            block_env,
            op_block_ctx,
        }
    }

    pub fn from_preflight(partial: PartialExecution, execution: &Execution) -> Self {
        let transactions = execution
            .get_transactions(&partial.tx_hashes)
            .into_iter()
            .map(|tx| tx.to_vec())
            .collect();
        Self::new(partial, transactions)
    }
}

/// Shared trace buffer
pub type TransactionResultCollector = Arc<Mutex<Vec<Vec<(B256, PartialResultAndState)>>>>;

/// EVM wrapper that serves pre-computed `ResultAndState` entries
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
    /// Wraps `inner` and prepares the chunk cache
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
            log(&format!("CACHED {incoming_hash}"));
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
            let result = self.evm.transact_raw(tx);

            // Capture result. We clone the revm result for the trait return,
            // and drain the clone into a `PartialResultAndState` (sorted Vec
            // form) for the trace buffer.
            if let (Ok(r), Some(traces)) = (&result, &self.collection_target) {
                let mut guard = traces.lock().unwrap();
                guard
                    .last_mut()
                    .expect(
                        "CachedEvmFactory pushes an empty slot before constructing \
                     a CachedEvm; last_mut() must exist",
                    )
                    .push((incoming_hash, PartialResultAndState::from(r.clone())));
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
    pub fn take_all_block_traces(&self) -> Vec<Vec<(B256, PartialResultAndState)>> {
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use crate::evm::CachedEvmFactory;
    use crate::evm::PartialExecution;
    use crate::evm::PartialExecutionWitness;
    use crate::evm::PartialResultAndState;
    use alloy_evm::op_revm::{OpHaltReason, OpSpecId, OpTransaction};
    use alloy_evm::revm::context::CfgEnv;
    use alloy_evm::revm::context::{BlockEnv, TxEnv};
    use alloy_evm::revm::context_interface::result::ResultAndState;
    use alloy_evm::revm::context_interface::result::{ExecutionResult, Output, SuccessReason};
    use alloy_evm::revm::database::in_memory_db::InMemoryDB;
    use alloy_evm::revm::database::states::CacheAccount;
    use alloy_evm::revm::database::CacheState;
    use alloy_evm::revm::state::Account;
    use alloy_evm::revm::state::AccountInfo;
    use alloy_evm::revm::state::Bytecode;
    use alloy_evm::{Evm, EvmEnv, EvmFactory};
    use alloy_op_evm::block::OpBlockExecutionCtx;
    use alloy_primitives::Address;
    use alloy_primitives::{address, keccak256, TxKind, U256};
    use alloy_primitives::{Bytes, B256};

    fn test_env_for_block(block_number: u64) -> EvmEnv<OpSpecId> {
        let block_env = BlockEnv {
            number: U256::from(block_number),
            ..Default::default()
        };
        let mut cfg_env = CfgEnv::default();
        cfg_env.chain_id = 1;
        cfg_env.spec = OpSpecId::BEDROCK;
        EvmEnv { block_env, cfg_env }
    }

    fn make_transfer(
        caller: Address,
        to: Address,
        value: U256,
        nonce: u64,
    ) -> OpTransaction<TxEnv> {
        OpTransaction {
            base: TxEnv {
                caller,
                kind: TxKind::Call(to),
                value,
                gas_limit: 21000,
                gas_price: 1,
                nonce,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn stub_result_and_state(gas_used: u64) -> ResultAndState<OpHaltReason> {
        ResultAndState {
            result: ExecutionResult::Success {
                reason: SuccessReason::Return,
                gas_used,
                gas_refunded: 0,
                logs: vec![],
                output: Output::Call(alloy_primitives::Bytes::new()),
            },
            state: Default::default(),
        }
    }

    fn stub_chunk(block_number: u64, gas_used_markers: &[u64]) -> PartialExecution {
        let default_tx_hash = keccak256([0x00u8]);
        let block_env = BlockEnv {
            number: U256::from(block_number),
            ..Default::default()
        };
        PartialExecution {
            tx_hashes: vec![default_tx_hash; gas_used_markers.len()],
            results: gas_used_markers
                .iter()
                .copied()
                .map(stub_result_and_state)
                .map(PartialResultAndState::from)
                .collect(),
            block_env,
            op_block_ctx: alloy_op_evm::block::OpBlockExecutionCtx::default(),
        }
    }

    #[test]
    fn precomputed_results_returned_in_order() {
        let chunks = vec![vec![stub_chunk(1, &[100_001, 100_002, 100_003])]];
        let factory = CachedEvmFactory::new(chunks);

        let sender = address!("0x1000000000000000000000000000000000000000");
        let recipient = address!("0x2000000000000000000000000000000000000000");
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            sender,
            AccountInfo {
                balance: U256::from(1_000_000_000_000_000_000u128),
                nonce: 0,
                ..Default::default()
            },
        );

        let mut evm = factory.create_evm(db, test_env_for_block(1));

        for expected in [100_001u64, 100_002, 100_003] {
            let result = evm
                .transact_raw(make_transfer(sender, recipient, U256::from(1), 0))
                .unwrap();
            match result.result {
                ExecutionResult::Success { gas_used, .. } => assert_eq!(gas_used, expected),
                _ => panic!("expected pre-computed Success result"),
            }
            // Pre-computed state is empty — a live transfer would diff two accounts.
            assert!(
                result.state.is_empty(),
                "pre-computed state should be empty"
            );
        }
    }

    #[test]
    fn empty_chunks_delegate_to_inner() {
        let factory = CachedEvmFactory::new(Vec::new());

        let sender = address!("0x1000000000000000000000000000000000000000");
        let recipient = address!("0x2000000000000000000000000000000000000000");
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            sender,
            AccountInfo {
                balance: U256::from(1_000_000_000_000_000_000u128),
                nonce: 0,
                ..Default::default()
            },
        );

        let mut evm = factory.create_evm(db, test_env_for_block(1));
        let result = evm
            .transact_raw(make_transfer(sender, recipient, U256::from(1000), 0))
            .unwrap();

        // Live execution must populate both accounts in state.
        assert!(result.state.contains_key(&sender));
        assert!(result.state.contains_key(&recipient));
    }

    #[test]
    fn system_calls_delegate_and_do_not_advance_tx_index() {
        let chunks = vec![vec![stub_chunk(1, &[42])]];
        let factory = CachedEvmFactory::new(chunks);

        let db = InMemoryDB::default();
        let mut evm = factory.create_evm(db, test_env_for_block(1));

        // Simulate a prelude system call — tx_index must stay at 0.
        let caller = address!("0xdeaddeaddeaddeaddeaddeaddeaddeaddead0001");
        let contract = address!("0x000F3df6D732807Ef1319fB7B8bB8522d0Beac02");
        let _ = evm.transact_system_call(caller, contract, alloy_primitives::Bytes::new());

        // Now the first tx-body transaction should still get the chunk's result[0].
        let sender = address!("0x1000000000000000000000000000000000000000");
        let result = evm
            .transact_raw(make_transfer(sender, Address::ZERO, U256::ZERO, 0))
            .unwrap();
        match result.result {
            ExecutionResult::Success { gas_used, .. } => assert_eq!(gas_used, 42),
            _ => panic!("expected pre-computed Success result"),
        }
    }

    #[test]
    fn multi_chunk_within_block_crosses_boundary() {
        // Chunk 0: txs 0..2, markers [10, 20]. Chunk 1: txs 2..4, markers [30, 40].
        let chunks = vec![vec![stub_chunk(1, &[10, 20]), stub_chunk(1, &[30, 40])]];
        let factory = CachedEvmFactory::new(chunks);

        let db = InMemoryDB::default();
        let mut evm = factory.create_evm(db, test_env_for_block(1));

        for expected in [10u64, 20, 30, 40] {
            let sender = address!("0x1000000000000000000000000000000000000000");
            let result = evm
                .transact_raw(make_transfer(sender, Address::ZERO, U256::ZERO, 0))
                .unwrap();
            match result.result {
                ExecutionResult::Success { gas_used, .. } => assert_eq!(gas_used, expected),
                _ => panic!("expected pre-computed Success result"),
            }
        }
    }

    #[test]
    fn factory_serves_chunks_in_creation_order() {
        // Slot 0: empty (first EVM created gets no pre-computed results).
        // Slot 1: one chunk with marker 777 (second EVM gets the pre-computed result).
        // Second EVM is built with `test_env_for_block(5)`, so the chunk's
        // `block_env.number` must equal 5 to satisfy `CachedEvm::transact_raw`.
        let chunks = vec![vec![], vec![stub_chunk(5, &[777])]];
        let factory = CachedEvmFactory::new(chunks);

        // First create_evm → empty chunks → delegate to inner (live execution).
        let sender = address!("0x1000000000000000000000000000000000000000");
        let mut db_first = InMemoryDB::default();
        db_first.insert_account_info(
            sender,
            AccountInfo {
                balance: U256::from(1_000_000_000_000_000_000u128),
                nonce: 0,
                ..Default::default()
            },
        );
        let mut evm_first = factory.create_evm(db_first, test_env_for_block(7));
        let r_first = evm_first
            .transact_raw(make_transfer(sender, Address::ZERO, U256::from(1), 0))
            .unwrap();
        // Live execution — non-empty state.
        assert!(!r_first.state.is_empty());

        // Second create_evm → pops the next slot → returns the pre-computed result.
        let db_second = InMemoryDB::default();
        let mut evm_second = factory.create_evm(db_second, test_env_for_block(5));
        let r_second = evm_second
            .transact_raw(make_transfer(sender, Address::ZERO, U256::from(1), 0))
            .unwrap();
        match r_second.result {
            ExecutionResult::Success { gas_used, .. } => assert_eq!(gas_used, 777),
            _ => panic!("expected pre-computed Success result"),
        }
    }

    #[test]
    fn trait_method_field_access_works() {
        let factory = CachedEvmFactory::new(Vec::new());
        let db = InMemoryDB::default();
        let evm = factory.create_evm(db, test_env_for_block(42));
        assert_eq!(evm.block().number, U256::from(42));
        assert_eq!(evm.chain_id(), 1);
        let _ = evm.db();
    }

    #[test]
    fn take_next_chunks_pops_in_order() {
        // No EVM is built here — block_number is irrelevant, but pass a non-zero
        // value for consistency with other tests.
        let chunks = vec![vec![stub_chunk(1, &[123])]];
        let factory = CachedEvmFactory::new(chunks);

        // First call drains the single block's chunks.
        let drained = factory.take_next_chunks();
        assert_eq!(drained.len(), 1);
        // Second call returns empty.
        assert!(factory.take_next_chunks().is_empty());
    }

    #[test]
    fn exhausted_chunks_delegate_to_inner() {
        // Chunk has only one result.
        let chunks = vec![vec![stub_chunk(1, &[999])]];
        let factory = CachedEvmFactory::new(chunks);

        let sender = address!("0x1000000000000000000000000000000000000000");
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            sender,
            AccountInfo {
                balance: U256::from(1_000_000_000_000_000_000u128),
                nonce: 0,
                ..Default::default()
            },
        );
        let mut evm = factory.create_evm(db, test_env_for_block(1));

        // tx 0: pre-computed.
        let r0 = evm
            .transact_raw(make_transfer(sender, Address::ZERO, U256::from(1), 0))
            .unwrap();
        match r0.result {
            ExecutionResult::Success { gas_used, .. } => assert_eq!(gas_used, 999),
            _ => panic!("expected pre-computed Success result"),
        }
        assert!(r0.state.is_empty(), "pre-computed state should be empty");

        // tx 1: chunks exhausted, delegate — real state diff.
        let r1 = evm
            .transact_raw(make_transfer(sender, Address::ZERO, U256::from(1), 0))
            .unwrap();
        assert!(!r1.state.is_empty(), "delegated tx should have state diff");
    }

    #[test]
    fn partial_execution_witness_rkyv_round_trip() {
        let addr = address!("0x1111111111111111111111111111111111111111");
        let code = Bytecode::new_raw(Bytes::from_static(&[0x60, 0x00, 0x60, 0x00]));
        let code_hash = code.hash_slow();
        let mut cache = CacheState::default();
        cache
            .accounts
            .insert(addr, CacheAccount::new_loaded_not_existing());
        cache.contracts.insert(code_hash, code.clone());

        let block_env = BlockEnv {
            number: U256::from(123),
            beneficiary: address!("0x2222222222222222222222222222222222222222"),
            timestamp: U256::from(1_700_000_000u64),
            gas_limit: 30_000_000,
            basefee: 7,
            difficulty: U256::ZERO,
            prevrandao: Some(B256::repeat_byte(0xAA)),
            ..Default::default()
        };

        let op_block_ctx = OpBlockExecutionCtx {
            parent_hash: B256::repeat_byte(0xBB),
            parent_beacon_block_root: Some(B256::repeat_byte(0xCC)),
            extra_data: Bytes::from_static(&[0xDE, 0xAD, 0xBE, 0xEF]),
        };

        let witness = PartialExecutionWitness {
            transactions: vec![vec![0x01, 0x02, 0x03], Vec::new(), vec![0xff; 32]],
            cache,
            block_env,
            op_block_ctx,
        };

        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&witness).unwrap();
        let recoded =
            rkyv::from_bytes::<PartialExecutionWitness, rkyv::rancor::Error>(&bytes).unwrap();

        // transactions: Vec<Vec<u8>> — direct equality.
        assert_eq!(witness.transactions, recoded.transactions);

        // BlockEnv derives PartialEq.
        assert_eq!(witness.block_env, recoded.block_env);

        // OpBlockExecutionCtx does not derive PartialEq — compare its three fields,
        // matching the pattern used by `op_block_execution_ctx_round_trip` in
        // `rkyv/evm.rs`.
        assert_eq!(
            witness.op_block_ctx.parent_hash,
            recoded.op_block_ctx.parent_hash
        );
        assert_eq!(
            witness.op_block_ctx.parent_beacon_block_root,
            recoded.op_block_ctx.parent_beacon_block_root
        );
        assert_eq!(
            witness.op_block_ctx.extra_data,
            recoded.op_block_ctx.extra_data
        );

        // Cache: verify the inserted entries survived and shape-level fields match.
        assert_eq!(recoded.cache.accounts.len(), 1);
        assert!(recoded.cache.accounts.contains_key(&addr));
        assert_eq!(recoded.cache.contracts.len(), 1);
        assert_eq!(
            recoded
                .cache
                .contracts
                .get(&code_hash)
                .map(|b| b.original_bytes()),
            Some(code.original_bytes())
        );
        assert_eq!(witness.cache.has_state_clear, recoded.cache.has_state_clear);
    }

    #[test]
    fn partial_account_revm_round_trip() {
        use crate::evm::PartialAccount;
        use alloy_evm::revm::primitives::HashMap;
        use alloy_evm::revm::state::{AccountInfo, AccountStatus, EvmStorageSlot};

        let info = AccountInfo {
            nonce: 7,
            balance: U256::from(123_456u64),
            code_hash: B256::repeat_byte(0xAB),
            account_id: None,
            code: None,
        };
        let original_info = AccountInfo {
            nonce: 5,
            balance: U256::from(100_000u64),
            code_hash: B256::repeat_byte(0xCD),
            account_id: None,
            code: None,
        };

        // Insert in reverse-sorted order to exercise the sort invariant.
        let mut storage: HashMap<U256, EvmStorageSlot> = Default::default();
        storage.insert(
            U256::from(42),
            EvmStorageSlot::new_changed(U256::ZERO, U256::from(99), 0),
        );
        storage.insert(
            U256::from(7),
            EvmStorageSlot::new_changed(U256::from(1), U256::from(2), 0),
        );
        storage.insert(
            U256::from(13),
            EvmStorageSlot::new_changed(U256::from(3), U256::from(4), 0),
        );

        let account = Account {
            info,
            original_info: Box::new(original_info),
            transaction_id: 99,
            storage,
            status: AccountStatus::Touched | AccountStatus::Created,
        };

        // Capture an independent reference for set-equality comparison after
        // round-trip.
        let original_storage_set: std::collections::BTreeSet<(U256, U256, U256)> = account
            .storage
            .iter()
            .map(|(k, v)| (*k, v.original_value, v.present_value))
            .collect();

        let partial = PartialAccount::from(account);

        // Sort invariant on the Vec.
        assert!(
            partial.storage.windows(2).all(|w| w[0].slot < w[1].slot),
            "PartialAccount::from(Account) must produce storage sorted by slot",
        );
        assert_eq!(partial.storage.len(), 3);

        // Round-trip back to revm.
        let rebuilt: Account = partial.into();
        assert_eq!(rebuilt.info.nonce, 7);
        assert_eq!(rebuilt.original_info.nonce, 5);
        assert_eq!(rebuilt.transaction_id, 99);
        assert_eq!(rebuilt.storage.len(), 3);
        let rebuilt_set: std::collections::BTreeSet<(U256, U256, U256)> = rebuilt
            .storage
            .iter()
            .map(|(k, v)| (*k, v.original_value, v.present_value))
            .collect();
        assert_eq!(original_storage_set, rebuilt_set);
    }

    #[test]
    fn partial_result_and_state_rkyv_round_trip() {
        use crate::evm::PartialResultAndState;
        use alloy_evm::revm::primitives::HashMap;
        use alloy_evm::revm::state::{AccountInfo, AccountStatus, EvmStorageSlot};

        let make_account = |seed: u8| {
            let mut storage: HashMap<U256, EvmStorageSlot> = Default::default();
            storage.insert(
                U256::from(2u64),
                EvmStorageSlot::new_changed(U256::ZERO, U256::from(seed as u64 + 100), 0),
            );
            storage.insert(
                U256::from(1u64),
                EvmStorageSlot::new_changed(U256::ZERO, U256::from(seed as u64 + 200), 0),
            );
            Account {
                info: AccountInfo {
                    nonce: seed as u64,
                    balance: U256::from(seed as u64 * 1000),
                    code_hash: B256::repeat_byte(seed),
                    account_id: None,
                    code: None,
                },
                original_info: Box::new(AccountInfo {
                    nonce: 0,
                    balance: U256::ZERO,
                    code_hash: B256::ZERO,
                    account_id: None,
                    code: None,
                }),
                transaction_id: seed as usize,
                storage,
                status: AccountStatus::Touched,
            }
        };

        let mut state: alloy_evm::revm::primitives::HashMap<Address, Account> = Default::default();
        // Insert out of address order.
        state.insert(
            address!("0xCCCC000000000000000000000000000000000000"),
            make_account(0xCC),
        );
        state.insert(
            address!("0xAAAA000000000000000000000000000000000000"),
            make_account(0xAA),
        );
        state.insert(
            address!("0xBBBB000000000000000000000000000000000000"),
            make_account(0xBB),
        );

        let revm_ras = ResultAndState {
            result: ExecutionResult::Success {
                reason: SuccessReason::Return,
                gas_used: 50_000,
                gas_refunded: 0,
                logs: vec![],
                output: Output::Call(Bytes::new()),
            },
            state,
        };

        let partial = PartialResultAndState::from(revm_ras);

        // Sort invariants.
        assert!(
            partial
                .state
                .windows(2)
                .all(|w| w[0].address < w[1].address),
            "state must be sorted by address",
        );
        for entry in &partial.state {
            assert!(
                entry
                    .account
                    .storage
                    .windows(2)
                    .all(|w| w[0].slot < w[1].slot),
                "per-account storage must be sorted by slot",
            );
        }

        // rkyv round-trip.
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&partial).unwrap();
        let recoded: PartialResultAndState =
            rkyv::from_bytes::<PartialResultAndState, rkyv::rancor::Error>(&bytes).unwrap();

        // Sort invariants survive the round-trip.
        assert!(recoded
            .state
            .windows(2)
            .all(|w| w[0].address < w[1].address));
        for entry in &recoded.state {
            assert!(entry
                .account
                .storage
                .windows(2)
                .all(|w| w[0].slot < w[1].slot));
        }

        // Field equality. ExecutionResult success-variant fields:
        match (&partial.result, &recoded.result) {
            (
                ExecutionResult::Success { gas_used: a, .. },
                ExecutionResult::Success { gas_used: b, .. },
            ) => assert_eq!(a, b),
            _ => panic!("ExecutionResult variant changed across rkyv round-trip"),
        }
        assert_eq!(partial.state.len(), recoded.state.len());
        for (a_entry, b_entry) in partial.state.iter().zip(recoded.state.iter()) {
            assert_eq!(a_entry.address, b_entry.address);
            let a_acc = &a_entry.account;
            let b_acc = &b_entry.account;
            assert_eq!(a_acc.info.nonce, b_acc.info.nonce);
            assert_eq!(a_acc.info.balance, b_acc.info.balance);
            assert_eq!(a_acc.info.code_hash, b_acc.info.code_hash);
            assert_eq!(a_acc.original_info.nonce, b_acc.original_info.nonce);
            assert_eq!(a_acc.status.bits(), b_acc.status.bits());
            assert_eq!(a_acc.storage.len(), b_acc.storage.len());
            for (sa, sb) in a_acc.storage.iter().zip(b_acc.storage.iter()) {
                assert_eq!(sa.slot, sb.slot);
                assert_eq!(sa.slot_value.original_value, sb.slot_value.original_value);
                assert_eq!(sa.slot_value.present_value, sb.slot_value.present_value);
            }
        }

        // Round-trip to revm: HashMap rebuild must preserve set-equality of
        // (addr, slot, original, present) tuples.
        let rebuilt: ResultAndState<OpHaltReason> = recoded.into();
        assert_eq!(rebuilt.state.len(), partial.state.len());
        for entry in &partial.state {
            let rebuilt_acc = rebuilt
                .state
                .get(&entry.address)
                .expect("rebuilt state missing address");
            assert_eq!(rebuilt_acc.storage.len(), entry.account.storage.len());
            for slot_entry in &entry.account.storage {
                let rebuilt_v = rebuilt_acc
                    .storage
                    .get(&slot_entry.slot)
                    .expect("rebuilt account missing slot");
                assert_eq!(
                    rebuilt_v.original_value,
                    slot_entry.slot_value.original_value
                );
                assert_eq!(rebuilt_v.present_value, slot_entry.slot_value.present_value);
            }
        }
    }
}

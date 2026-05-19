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

use crate::boot::L1_HEAD_TXN_ONLY_SENTINEL;
use crate::evm::expected::{
    apply_result_to_expected_state, canonicalize_expected_state, ExpectedStateEntry,
};
use crate::precondition::evm::{
    compute_pe_trace, hash_block_ctx, hash_expected_state, hash_results,
};
use crate::rkyv::evm::{
    AccountInfoRkyv, AccountStatusRkyv, BlockEnvRkyv, EvmStorageSlotRkyv, ExecutionResultRkyv,
    OpBlockExecutionCtxRkyv,
};
use crate::rkyv::primitives::{AddressDef, B256Def, U256Def};
use alloy_evm::op_revm::OpHaltReason;
use alloy_evm::revm::context::result::{ExecutionResult, ResultAndState};
use alloy_evm::revm::context::BlockEnv;
use alloy_evm::revm::state::{Account, AccountInfo, AccountStatus, EvmStorageSlot};
use alloy_op_evm::OpBlockExecutionCtx;
use alloy_primitives::{Address, B256, U256};
use kona_proof::BootInfo;
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

#[derive(Clone, Debug)]
pub struct PartialExecutionTrace {
    pub tx_hash: B256,
    pub result: PartialResultAndState,
    pub expected_state: Vec<ExpectedStateEntry>,
}

/// Shared trace buffer
pub type TransactionResultCollector = Arc<Mutex<Vec<Vec<PartialExecutionTrace>>>>;

/// Represents a proven transaction subsequence within a block.
#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct PartialExecution {
    /// The EIP-2718 tx hash for each entry in `results`.
    #[rkyv(with = rkyv::with::Map<B256Def>)]
    pub tx_hashes: Vec<B256>,
    /// Full per-tx execution results (ExecutionResult + sorted state)
    pub results: Vec<PartialResultAndState>,
    /// State that is read by OP execution logic outside the returned EVM tx state.
    pub expected_state: Vec<ExpectedStateEntry>,
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
            hash_expected_state(&self.expected_state),
        )
    }

    pub fn boot_info(&self, boot: &BootInfo) -> BootInfo {
        BootInfo {
            l1_head: L1_HEAD_TXN_ONLY_SENTINEL,
            agreed_l2_output_root: self.op_block_ctx.parent_hash,
            claimed_l2_output_root: self.op_block_ctx.parent_hash,
            claimed_l2_block_number: self.block_env.number.to::<u64>().saturating_sub(1),
            chain_id: boot.chain_id,
            rollup_config: boot.rollup_config.clone(),
            l1_config: boot.l1_config.clone(),
        }
    }

    pub fn split(self, partials_per_block: usize) -> Vec<PartialExecution> {
        // return nothing if we don't want any partials per block
        if partials_per_block == 0 {
            return vec![];
        }

        // Flatten all executions, carrying the expected prestate at each tx boundary.
        let mut flattened = Vec::new();
        let mut running_expected_state = canonicalize_expected_state(self.expected_state);
        for (tx_hash, result) in self.tx_hashes.into_iter().zip(self.results) {
            // Push this result with expected state so far
            flattened.push((tx_hash, result.clone(), running_expected_state.clone()));
            // Update new expected state with any relevant results
            apply_result_to_expected_state(&mut running_expected_state, &result);
        }

        // Split into specified partial count
        let partial_size = flattened.len().div_ceil(partials_per_block).max(1);
        flattened
            .chunks(partial_size)
            .map(|chunk| PartialExecution {
                tx_hashes: chunk.iter().map(|(tx_hash, _, _)| *tx_hash).collect(),
                results: chunk.iter().map(|(_, result, _)| result.clone()).collect(),
                expected_state: chunk
                    .first()
                    .map(|(_, _, expected_state)| expected_state.clone())
                    .unwrap_or_default(),
                block_env: self.block_env.clone(),
                op_block_ctx: self.op_block_ctx.clone(),
            })
            .collect()
    }
}

pub struct ActivePartialExecution {
    pub partial: PartialExecution,
    pub expected_state_verified: bool,
}

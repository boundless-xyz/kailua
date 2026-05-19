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

use crate::evm::expected::ExpectedStateEntry;
use crate::evm::partial::{PartialExecution, PartialResultAndState, PartialStateEntry};
use crate::executor::Execution;
use crate::rkyv::evm::{BlockEnvRkyv, CacheStateRkyv, OpBlockExecutionCtxRkyv};
use alloy_evm::revm::context::BlockEnv;
use alloy_evm::revm::database::states::CacheAccount;
use alloy_evm::revm::database::CacheState;
use alloy_evm::revm::state::AccountStatus;
use alloy_op_evm::OpBlockExecutionCtx;

/// Witness data for proving a single transaction subsequence within a block.
///
/// `expected_state` is not carried explicitly: `cache_results` seeds the
/// host's snapshot into `cache` during construction (alongside this chunk's
/// own per-tx prestate), and the guest re-derives the snapshot via
/// `capture_required_expected_state` against `cache` when it needs to
/// compute `hash_expected_state`. The splitter keeps `expected_state`
/// bounded by `required_l1_block_slots_for_spec` plus the conditional
/// Ecotone L1_OVERHEAD, so the spec-bounded re-derive matches the host's
/// pre-hash input by construction.
#[derive(Clone, Debug, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct PartialExecutionWitness {
    /// List of transactions to execute
    pub transactions: Vec<Vec<u8>>,
    /// Pre-state cache (also implicitly carries the L1Block expected_state
    /// snapshot — see `cache_results`).
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
            expected_state,
            block_env,
            op_block_ctx,
            ..
        } = partial_execution;

        PartialExecutionWitness {
            transactions,
            cache: cache_results(results, expected_state),
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

pub fn cache_results(
    results: Vec<PartialResultAndState>,
    expected_state: Vec<ExpectedStateEntry>,
) -> CacheState {
    let mut cache_state = CacheState::default();
    // cache expected state
    for entry in expected_state {
        let storage = entry
            .account
            .storage
            .into_iter()
            .map(|entry| (entry.slot, entry.value))
            .collect();
        if entry.account.exists {
            if let Some(code) = entry.account.info.code.clone() {
                cache_state
                    .contracts
                    .entry(entry.account.info.code_hash)
                    .or_insert(code);
            }
            cache_state
                .accounts
                .entry(entry.address)
                .or_insert_with(|| CacheAccount::new_loaded(entry.account.info, storage));
        } else {
            cache_state
                .accounts
                .entry(entry.address)
                .or_insert_with(CacheAccount::new_loaded_not_existing);
        }
    }
    // cache results
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

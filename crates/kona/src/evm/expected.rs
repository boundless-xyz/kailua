// Copyright 2026 Boundless Foundation, Inc.
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

use crate::evm::partial::PartialResultAndState;
use crate::rkyv::evm::AccountInfoRkyv;
use crate::rkyv::primitives::{AddressDef, U256Def};
use alloy_evm::revm::Database as RevmDatabase;
use alloy_evm::revm::state::{AccountInfo, AccountStatus};
use alloy_primitives::{Address, U256};
use op_revm::constants::{
    ECOTONE_L1_BLOB_BASE_FEE_SLOT, ECOTONE_L1_FEE_SCALARS_SLOT, L1_BASE_FEE_SLOT,
    L1_BLOCK_CONTRACT, L1_OVERHEAD_SLOT, L1_SCALAR_SLOT, OPERATOR_FEE_SCALARS_SLOT,
};
use std::collections::BTreeMap;
use std::mem::take;

/// A storage slot and its expected value.
#[derive(Clone, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ExpectedStorageEntry {
    /// The storage slot key.
    #[rkyv(with = U256Def)]
    pub slot: U256,
    /// The value expected in the slot.
    #[rkyv(with = U256Def)]
    pub value: U256,
}

/// The expected view of an account read by OP execution logic outside transaction state.
#[derive(Clone, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ExpectedAccount {
    /// Whether the account exists.
    pub exists: bool,
    /// The account's balance, nonce, and code identity.
    #[rkyv(with = AccountInfoRkyv)]
    pub info: AccountInfo,
    /// Sorted by slot key.
    pub storage: Vec<ExpectedStorageEntry>,
}

/// An account's expected state, keyed by address.
#[derive(Clone, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ExpectedStateEntry {
    /// The account's address.
    #[rkyv(with = AddressDef)]
    pub address: Address,
    /// The account's expected state.
    pub account: ExpectedAccount,
}

/// The accounts OP execution logic reads outside individual transaction state: the L1Block
/// predeploy.
pub const EXPECTED_STATE_ADDRESSES: [Address; 1] = [L1_BLOCK_CONTRACT];

/// The L1Block storage slots read by OP fee calculations.
pub const EXPECTED_STORAGE_SLOTS: [U256; 6] = [
    L1_BASE_FEE_SLOT,              // 1
    ECOTONE_L1_FEE_SCALARS_SLOT,   // 3
    L1_OVERHEAD_SLOT,              // 5
    L1_SCALAR_SLOT,                // 6
    ECOTONE_L1_BLOB_BASE_FEE_SLOT, // 7
    OPERATOR_FEE_SCALARS_SLOT,     // 8
                                   // DA_FOOTPRINT_GAS_SCALAR_SLOT,
];

/// Sorts the entries by address, and each account's storage by slot, for deterministic hashing.
pub fn canonicalize_expected_state(
    mut expected_state: Vec<ExpectedStateEntry>,
) -> Vec<ExpectedStateEntry> {
    expected_state.sort_by_key(|e| e.address);
    for entry in &mut expected_state {
        entry.account.storage.sort_by_key(|s| s.slot);
    }
    expected_state
}

/// Reads the fixed expected-state address and slot set out of the database, panicking if an
/// account read fails.
pub fn capture_required_expected_state<DB: RevmDatabase>(db: &mut DB) -> Vec<ExpectedStateEntry> {
    EXPECTED_STATE_ADDRESSES
        .into_iter()
        .map(|address| {
            let info = RevmDatabase::basic(db, address)
                .map_err(|_| ())
                .expect("capture_required_expected_state: DB basic read failed");
            let storage: Vec<_> = EXPECTED_STORAGE_SLOTS
                .iter()
                .copied()
                .map(|slot| ExpectedStorageEntry {
                    slot,
                    value: RevmDatabase::storage(db, address, slot).unwrap_or_default(),
                })
                .collect();
            ExpectedStateEntry {
                address,
                account: ExpectedAccount {
                    exists: info.is_some(),
                    info: info.unwrap_or_default(),
                    storage,
                },
            }
        })
        .collect()
}

/// Advance the running `expected_state` snapshot by one tx's result.
pub fn apply_result_to_expected_state(
    expected_state: &mut Vec<ExpectedStateEntry>,
    result: &PartialResultAndState,
) {
    let mut expected_by_address: BTreeMap<Address, ExpectedStateEntry> = take(expected_state)
        .into_iter()
        .map(|e| (e.address, e))
        .collect();

    for result_entry in result.state.iter() {
        // skip irrelevant entries
        if !EXPECTED_STATE_ADDRESSES.contains(&result_entry.address) {
            continue;
        }
        // Only update an entry that's already in expected_state — never
        // insert a new one. See the doc comment above for the rationale.
        let Some(expected_entry) = expected_by_address.get_mut(&result_entry.address) else {
            continue;
        };
        // update info / exists from the tx's poststate
        expected_entry.account.exists = !result_entry
            .account
            .status
            .contains(AccountStatus::LoadedAsNotExisting);
        expected_entry.account.info = result_entry.account.info.clone();
        // Only update slots that were already in expected_state — never
        // add new ones. Out-of-journal reads target a fixed slot set per
        // spec; new slots aren't part of that set.
        for result_slot in &result_entry.account.storage {
            if let Some(expected_slot) = expected_entry
                .account
                .storage
                .iter_mut()
                .find(|slot_entry| slot_entry.slot == result_slot.slot)
            {
                expected_slot.value = result_slot.slot_value.present_value;
            }
        }
    }
    // resort values
    *expected_state = canonicalize_expected_state(expected_by_address.into_values().collect());
}

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

use crate::evm::partial::PartialResultAndState;
use crate::rkyv::evm::AccountInfoRkyv;
use crate::rkyv::primitives::{AddressDef, U256Def};
use alloy_evm::op_revm::constants::{
    BASE_FEE_SCALAR_OFFSET, BLOB_BASE_FEE_SCALAR_OFFSET, DA_FOOTPRINT_GAS_SCALAR_SLOT,
    ECOTONE_L1_BLOB_BASE_FEE_SLOT, ECOTONE_L1_FEE_SCALARS_SLOT, EMPTY_SCALARS, L1_BASE_FEE_SLOT,
    L1_BLOCK_CONTRACT, L1_OVERHEAD_SLOT, L1_SCALAR_SLOT, OPERATOR_FEE_SCALARS_SLOT,
};
use alloy_evm::op_revm::OpSpecId;
use alloy_evm::revm::state::{AccountInfo, AccountStatus};
use alloy_evm::revm::Database as RevmDatabase;
use alloy_primitives::{Address, U256};
use std::collections::BTreeMap;
use std::mem::take;

#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ExpectedStorageEntry {
    #[rkyv(with = U256Def)]
    pub slot: U256,
    #[rkyv(with = U256Def)]
    pub value: U256,
}

#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ExpectedAccount {
    pub exists: bool,
    #[rkyv(with = AccountInfoRkyv)]
    pub info: AccountInfo,
    /// Sorted by slot key.
    pub storage: Vec<ExpectedStorageEntry>,
}

#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ExpectedStateEntry {
    #[rkyv(with = AddressDef)]
    pub address: Address,
    pub account: ExpectedAccount,
}

pub fn required_l1_block_slots() -> Vec<U256> {
    let mut slots = vec![
        L1_BASE_FEE_SLOT,
        L1_OVERHEAD_SLOT,
        L1_SCALAR_SLOT,
        ECOTONE_L1_BLOB_BASE_FEE_SLOT,
        ECOTONE_L1_FEE_SCALARS_SLOT,
        OPERATOR_FEE_SCALARS_SLOT,
        DA_FOOTPRINT_GAS_SCALAR_SLOT,
    ];
    slots.sort();
    slots.dedup();
    slots
}

pub fn required_l1_block_slots_for_spec(spec_id: OpSpecId) -> Vec<U256> {
    let mut slots = vec![L1_BASE_FEE_SLOT];
    if spec_id.is_enabled_in(OpSpecId::ECOTONE) {
        slots.push(ECOTONE_L1_BLOB_BASE_FEE_SLOT);
        slots.push(ECOTONE_L1_FEE_SCALARS_SLOT);
        if spec_id.is_enabled_in(OpSpecId::ISTHMUS) {
            slots.push(OPERATOR_FEE_SCALARS_SLOT);
        }
        if spec_id.is_enabled_in(OpSpecId::JOVIAN) {
            slots.push(DA_FOOTPRINT_GAS_SCALAR_SLOT);
        }
    } else {
        slots.push(L1_OVERHEAD_SLOT);
        slots.push(L1_SCALAR_SLOT);
    }
    slots.sort();
    slots.dedup();
    slots
}

fn ecotone_fee_scalars_need_overhead(l1_fee_scalars: U256) -> bool {
    let scalars = l1_fee_scalars.to_be_bytes::<32>();
    let l1_blob_base_fee = U256::from_be_slice(
        scalars[BLOB_BASE_FEE_SCALAR_OFFSET..BLOB_BASE_FEE_SCALAR_OFFSET + 4].as_ref(),
    );
    l1_blob_base_fee.is_zero()
        && scalars[BASE_FEE_SCALAR_OFFSET..BLOB_BASE_FEE_SCALAR_OFFSET + 4] == EMPTY_SCALARS
}

fn required_l1_block_slots_for_expected_state(
    spec_id: OpSpecId,
    expected_state: &[ExpectedStateEntry],
) -> Vec<U256> {
    let mut slots = required_l1_block_slots_for_spec(spec_id);
    if spec_id.is_enabled_in(OpSpecId::ECOTONE)
        && expected_state
            .iter()
            .find(|entry| entry.address == L1_BLOCK_CONTRACT)
            .and_then(|entry| {
                entry
                    .account
                    .storage
                    .iter()
                    .find(|slot_entry| slot_entry.slot == ECOTONE_L1_FEE_SCALARS_SLOT)
            })
            .is_some_and(|slot_entry| ecotone_fee_scalars_need_overhead(slot_entry.value))
    {
        slots.push(L1_OVERHEAD_SLOT);
        slots.sort();
        slots.dedup();
    }
    slots
}

pub const EXPECTED_STATE_ADDRESSES: [Address; 1] = [L1_BLOCK_CONTRACT];

pub fn canonicalize_expected_state(
    mut expected_state: Vec<ExpectedStateEntry>,
) -> Vec<ExpectedStateEntry> {
    expected_state.sort_by_key(|e| e.address);
    for entry in &mut expected_state {
        entry.account.storage.sort_by_key(|s| s.slot);
    }
    expected_state
}

pub fn validate_expected_state_coverage(
    expected_state: &[ExpectedStateEntry],
    required_slots: &[U256],
) {
    for required_address in EXPECTED_STATE_ADDRESSES {
        let entry = expected_state
            .iter()
            .find(|entry| entry.address == required_address)
            .unwrap_or_else(|| {
                panic!(
                    "PartialExecution expected_state missing required address {required_address}"
                )
            });
        for &required_slot in required_slots {
            assert!(
                entry
                    .account
                    .storage
                    .iter()
                    .any(|slot_entry| slot_entry.slot == required_slot),
                "PartialExecution expected_state missing required slot {required_slot} for address {required_address}",
            );
        }
    }
}

fn info_matches(actual: &AccountInfo, expected: &AccountInfo) -> bool {
    actual.nonce == expected.nonce
        && actual.balance == expected.balance
        && actual.code_hash == expected.code_hash
}

pub fn verify_expected_state<DB: RevmDatabase>(
    db: &mut DB,
    expected_state: &[ExpectedStateEntry],
    spec_id: OpSpecId,
) {
    let required_slots = required_l1_block_slots_for_expected_state(spec_id, expected_state);
    validate_expected_state_coverage(expected_state, &required_slots);
    for entry in expected_state {
        let actual_info = RevmDatabase::basic(db, entry.address)
            .map_err(|_| ())
            .expect("verify_expected_state: DB basic read failed");
        if entry.account.exists {
            let actual_info = actual_info.as_ref().unwrap_or_else(|| {
                panic!(
                    "verify_expected_state: expected account {} to exist",
                    entry.address
                )
            });
            assert!(
                info_matches(actual_info, &entry.account.info),
                "verify_expected_state: account mismatch at addr={}: expected {:?}, got {:?}",
                entry.address,
                entry.account.info,
                actual_info,
            );
        } else {
            assert!(
                actual_info.is_none(),
                "verify_expected_state: expected account {} to be absent, got {:?}",
                entry.address,
                actual_info,
            );
        }
        for slot_entry in &entry.account.storage {
            let actual = RevmDatabase::storage(db, entry.address, slot_entry.slot)
                .map_err(|_| ())
                .expect("verify_expected_state: DB storage read failed");
            assert_eq!(
                actual, slot_entry.value,
                "verify_expected_state: storage mismatch at addr={} slot={}: expected {} got {}",
                entry.address, slot_entry.slot, slot_entry.value, actual,
            );
        }
    }
}

pub fn capture_required_expected_state<DB: RevmDatabase>(
    db: &mut DB,
    spec_id: OpSpecId,
) -> Vec<ExpectedStateEntry> {
    let required_slots = required_l1_block_slots_for_spec(spec_id);
    EXPECTED_STATE_ADDRESSES
        .into_iter()
        .map(|address| {
            let info = RevmDatabase::basic(db, address)
                .map_err(|_| ())
                .expect("capture_required_expected_state: DB basic read failed");
            let mut storage: Vec<_> = required_slots
                .iter()
                .copied()
                .map(|slot| ExpectedStorageEntry {
                    slot,
                    value: RevmDatabase::storage(db, address, slot).unwrap_or_else(|_| {
                        panic!(
                            "capture_required_expected_state: DB storage read failed for address {address} slot {slot}"
                        )
                    }),
                })
                .collect();
            if spec_id.is_enabled_in(OpSpecId::ECOTONE)
                && storage
                    .iter()
                    .find(|slot_entry| slot_entry.slot == ECOTONE_L1_FEE_SCALARS_SLOT)
                    .is_some_and(|slot_entry| ecotone_fee_scalars_need_overhead(slot_entry.value))
            {
                storage.push(ExpectedStorageEntry {
                    slot: L1_OVERHEAD_SLOT,
                    value: RevmDatabase::storage(db, address, L1_OVERHEAD_SLOT).unwrap_or_else(
                        |_| {
                            panic!(
                                "capture_required_expected_state: DB storage read failed for address {address} slot {L1_OVERHEAD_SLOT}"
                            )
                        },
                    ),
                });
                storage.sort_by_key(|entry| entry.slot);
                storage.dedup_by_key(|entry| entry.slot);
            }
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
        // insert new entry if not existing
        let expected_entry = expected_by_address
            .entry(result_entry.address)
            .or_insert_with(|| ExpectedStateEntry {
                address: result_entry.address,
                account: ExpectedAccount {
                    // initialized below
                    info: AccountInfo::default(),
                    exists: Default::default(),
                    storage: Default::default(),
                    // storage: required_l1_block_slots()
                    //     .into_iter()
                    //     .map(|slot| PartialExpectedStorageEntry {
                    //         slot,
                    //         value: result_entry
                    //             .account
                    //             .storage
                    //             .iter()
                    //             .find(|slot_entry| slot_entry.slot == slot)
                    //             .map(|slot_entry| slot_entry.slot_value.original_value)
                    //             .unwrap_or_default(),
                    //     })
                    //     .collect(),
                },
            });
        // update/initialize entry
        expected_entry.account.exists = !result_entry
            .account
            .status
            .contains(AccountStatus::LoadedAsNotExisting);
        expected_entry.account.info = result_entry.account.info.clone();
        for result_slot in &result_entry.account.storage {
            if let Some(expected_slot) = expected_entry
                .account
                .storage
                .iter_mut()
                .find(|slot_entry| slot_entry.slot == result_slot.slot)
            {
                // update what we have
                expected_slot.value = result_slot.slot_value.present_value;
            } else {
                // add what we're missing
                expected_entry.account.storage.push(ExpectedStorageEntry {
                    slot: result_slot.slot,
                    value: result_slot.slot_value.present_value,
                });
            }
        }
    }
    // resort values
    *expected_state = canonicalize_expected_state(expected_by_address.into_values().collect());
}

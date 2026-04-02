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

use alloy_evm::revm::database::in_memory_db::{AccountState, Cache, DbAccount};
use alloy_evm::revm::database::states::cache::CacheState;
use alloy_evm::revm::primitives::{HashMap, KECCAK_EMPTY};
use alloy_evm::revm::state::{AccountInfo, AccountStatus, Bytecode, EvmState};
use alloy_primitives::{Address, Bloom, B256, U256};
use op_alloy_consensus::OpReceiptEnvelope;
use risc0_zkvm::sha::rust_crypto::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// EVM state accumulators tracked across chunk boundaries.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct EvmAccumulatorState {
    pub cumulative_gas_used: u64,
    pub da_footprint_used: u64,
    pub blob_gas_used: u64,
    pub logs_bloom: Bloom,
    #[rkyv(with = rkyv::with::Map<crate::rkyv::chunking::OpReceiptRlpRkyv>)]
    pub receipts: Vec<OpReceiptEnvelope>,
}

/// Maps an `AccountStatus` (from `CacheState`/`State`) to an `AccountState` (from `Cache`/`CacheDB`)
/// for canonical hashing only. This normalization ensures that both revm state representations
/// produce identical hashes for the same logical state.
///
/// The mapping is:
/// - `LoadedNotExisting`                        → `NotExisting` (account does not exist)
/// - `Loaded`, `LoadedEmptyEIP161`              → `None` (account exists, unmodified)
/// - `Changed`, `InMemoryChange`                → `Touched` (account was modified by EVM)
/// - `Destroyed`, `DestroyedChanged`, `DestroyedAgain` → `StorageCleared` (storage was wiped)
pub fn normalize_account_status(
    status: &alloy_evm::revm::database::states::AccountStatus,
) -> AccountState {
    use alloy_evm::revm::database::states::AccountStatus;
    match status {
        AccountStatus::LoadedNotExisting => AccountState::NotExisting,
        AccountStatus::Loaded | AccountStatus::LoadedEmptyEIP161 => AccountState::None,
        AccountStatus::Changed | AccountStatus::InMemoryChange => AccountState::Touched,
        AccountStatus::Destroyed
        | AccountStatus::DestroyedChanged
        | AccountStatus::DestroyedAgain => AccountState::StorageCleared,
    }
}

/// Encodes an `AccountState` as a single canonical byte for hashing.
pub fn account_state_byte(state: &AccountState) -> u8 {
    match state {
        AccountState::NotExisting => 0,
        AccountState::None => 1,
        AccountState::Touched => 2,
        AccountState::StorageCleared => 3,
    }
}

/// Feed a single `DbAccount` into a streaming hasher, sorting storage by slot key.
fn hash_account(hasher: &mut Sha256, addr: &Address, acct: &DbAccount) {
    hasher.update(addr.as_slice());
    hasher.update(acct.info.nonce.to_be_bytes());
    hasher.update(acct.info.balance.to_be_bytes::<32>());
    hasher.update(acct.info.code_hash.as_slice());
    hasher.update([account_state_byte(&acct.account_state)]);
    let mut sorted_storage: Vec<_> = acct.storage.iter().collect();
    sorted_storage.sort_by_key(|(k, _)| *k);
    hasher.update((sorted_storage.len() as u64).to_be_bytes());
    for (slot, value) in &sorted_storage {
        hasher.update(slot.to_be_bytes::<32>());
        hasher.update(value.to_be_bytes::<32>());
    }
}

/// Maps the bitflags-based [`AccountStatus`] (from `EvmState` traces) to the enum-based
/// [`AccountState`] (from `Cache`/`CacheDB`).
///
/// This is used by both host-side witness construction and guest-side chunk verification
/// when applying merged state deltas to advance the cumulative cache.
pub fn account_state_from_evm_status(status: AccountStatus) -> AccountState {
    if status.intersects(AccountStatus::SelfDestructed | AccountStatus::Created) {
        AccountState::StorageCleared
    } else if status.contains(AccountStatus::Touched) {
        AccountState::Touched
    } else {
        AccountState::None
    }
}

/// Applies a single transaction's [`EvmState`] trace (or a merged state delta) to a
/// cumulative [`Cache`], updating account info, storage, lifecycle state, and contract
/// bytecodes.
///
/// This is used by both host-side witness construction (advancing the cumulative cache
/// through each chunk's traces) and guest-side chunk verification (applying the merged
/// state delta from a [`Chunk`] proof to advance state during aggregation).
pub fn apply_trace_to_cache(cache: &mut Cache, trace: &EvmState) {
    for (addr, account) in trace {
        let db_account = cache.accounts.entry(*addr).or_insert_with(|| DbAccount {
            info: AccountInfo::default(),
            account_state: AccountState::NotExisting,
            storage: Default::default(),
        });

        db_account.info = account.info.clone();
        db_account.account_state = account_state_from_evm_status(account.status);

        // For created/self-destructed accounts, clear inherited storage
        if account
            .status
            .intersects(AccountStatus::SelfDestructed | AccountStatus::Created)
        {
            db_account.storage.clear();
        }

        // Overlay storage changes
        for (slot, evm_slot) in &account.storage {
            db_account.storage.insert(*slot, evm_slot.present_value);
        }

        // Update contracts if code is present
        if let Some(code) = &account.info.code {
            if account.info.code_hash != KECCAK_EMPTY {
                cache.contracts.insert(account.info.code_hash, code.clone());
            }
        }
    }
}

/// Validates that each cached contract bytecode hashes to its map key.
///
/// This is required because the canonical state hash encodes only the sorted set of contract
/// code-hash keys. If a witness provided arbitrary bytecode under a valid key and we failed to
/// check it first, the hash would authenticate the wrong executable code.
pub fn validate_cached_contracts(contracts: &HashMap<B256, Bytecode>) {
    for (expected_hash, bytecode) in contracts {
        let actual_hash = bytecode.hash_slow();
        assert_eq!(
            actual_hash, *expected_hash,
            "cached contract bytecode hash mismatch: expected {expected_hash:?}, got {actual_hash:?}"
        );
    }
}

/// Projects a `CacheAccount` (from `CacheState`/`State`) into a `DbAccount` (from `Cache`/`CacheDB`)
/// for canonical hashing. Extracts `AccountInfo` and storage from the inner `PlainAccount` if
/// present, or defaults to empty info and storage for non-existent accounts. The `AccountStatus`
/// is normalized to `AccountState` via [`normalize_account_status`].
fn projected_db_account(cache_acct: &alloy_evm::revm::database::states::CacheAccount) -> DbAccount {
    let account_state = normalize_account_status(&cache_acct.status);
    let (info, storage) = match &cache_acct.account {
        Some(plain) => (
            plain.info.clone(),
            plain.storage.iter().map(|(k, v)| (*k, *v)).collect(),
        ),
        None => (AccountInfo::default(), Default::default()),
    };

    DbAccount {
        info,
        account_state,
        storage,
    }
}

/// Compute a canonical state hash from a materialized `Cache`.
/// Used when a full `Cache` snapshot is available, e.g. a chunk witness snapshot
/// or a `CacheDB` that has been fully populated.
/// Excludes `Cache.logs` — logs are committed via the EVM accumulator (receipts/bloom).
pub fn hash_cache(cache: &Cache) -> B256 {
    hash_canonical(
        cache
            .accounts
            .iter()
            .map(|(addr, db_acct)| (*addr, db_acct)),
        cache.contracts.keys().copied(),
        cache
            .block_hashes
            .iter()
            .map(|(num, hash)| (*num, *hash))
            .collect(),
    )
}

/// Compute a canonical state hash from a `CacheState` + `State.block_hashes`.
/// Used in the aggregation guest where execution runs on `State<TrieDB>` — there is no
/// underlying preloaded `Cache` layer, so the `CacheState` plus block_hashes is itself the
/// complete flat view (e.g. hashing post-prelude state after `apply_pre_execution_changes()`).
pub fn hash_cache_state(cache_state: &CacheState, block_hashes: &BTreeMap<u64, B256>) -> B256 {
    let accounts: Vec<_> = cache_state
        .accounts
        .iter()
        .map(|(addr, cache_acct)| (*addr, projected_db_account(cache_acct)))
        .collect();
    hash_canonical(
        accounts.iter().map(|(addr, acct)| (*addr, acct)),
        cache_state.contracts.keys().copied(),
        block_hashes
            .iter()
            .map(|(num, hash)| (U256::from(*num), *hash))
            .collect(),
    )
}

/// Compute a canonical state hash from a `CacheState` overlaid onto an initial `Cache` base,
/// plus `State.block_hashes`. Used in chunk proving guests where execution runs on
/// `State<CacheDB<PanicDB>>` — the `CacheDB.cache` was preloaded from the chunk witness and
/// `CacheState` only contains entries actually touched during execution. Untouched witness
/// entries in the base `Cache` still contribute to the hash, ensuring the full logical
/// post-state is captured.
pub fn hash_overlay_state(
    base_cache: &Cache,
    live_state: &CacheState,
    block_hashes: &BTreeMap<u64, B256>,
) -> B256 {
    // Merge accounts: start from base, overlay live state
    let mut accounts = HashMap::<Address, DbAccount>::default();

    // Base layer from Cache
    for (addr, db_acct) in &base_cache.accounts {
        accounts.insert(*addr, db_acct.clone());
    }

    // Overlay live CacheState on top
    for (addr, cache_acct) in &live_state.accounts {
        let projected = projected_db_account(cache_acct);
        match projected.account_state {
            AccountState::StorageCleared | AccountState::NotExisting => {
                // Rebuild from the live state so wiped/deleted accounts do not retain inherited
                // base info or storage.
                accounts.insert(*addr, projected);
            }
            AccountState::None | AccountState::Touched => {
                let entry = accounts
                    .entry(*addr)
                    .or_insert_with(DbAccount::new_not_existing);
                entry.info = projected.info;
                entry.account_state = projected.account_state;
                // Overlay storage: live values replace base values
                for (slot, value) in projected.storage {
                    entry.storage.insert(slot, value);
                }
            }
        }
    }

    // Merge validated contract code hashes (keys only — validation authenticates the content)
    let mut contract_hashes = BTreeSet::new();
    contract_hashes.extend(base_cache.contracts.keys().copied());
    contract_hashes.extend(live_state.contracts.keys().copied());

    // Merge block_hashes: base + State.block_hashes overlay
    let mut merged_hashes: BTreeMap<U256, B256> = base_cache
        .block_hashes
        .iter()
        .map(|(num, hash)| (*num, *hash))
        .collect();
    for (num, hash) in block_hashes {
        merged_hashes.insert(U256::from(*num), *hash);
    }

    hash_canonical(
        accounts.iter().map(|(addr, acct)| (*addr, acct)),
        contract_hashes.into_iter(),
        merged_hashes,
    )
}

/// Core canonical hashing over account/contract/block_hash iterators.
/// Uses streaming SHA256 — no intermediate buffer allocation.
/// Canonical encoding:
///   accounts (sorted by Address) || contract code_hashes (sorted) || block_hashes (sorted by U256)
fn hash_canonical<'a>(
    accounts: impl Iterator<Item = (Address, &'a DbAccount)>,
    contract_hashes: impl Iterator<Item = B256>,
    block_hashes: BTreeMap<U256, B256>,
) -> B256 {
    let mut hasher = Sha256::new();

    // Sort accounts by Address
    let mut sorted_accounts: Vec<_> = accounts.collect();
    sorted_accounts.sort_by_key(|(addr, _)| *addr);

    // Encode accounts section
    hasher.update((sorted_accounts.len() as u64).to_be_bytes());
    for (addr, acct) in &sorted_accounts {
        hash_account(&mut hasher, addr, acct);
    }

    // Sort contract code_hashes (code_hash already commits to bytecode content)
    let mut sorted_hashes: Vec<_> = contract_hashes.collect();
    sorted_hashes.sort();

    // Encode contracts section
    hasher.update((sorted_hashes.len() as u64).to_be_bytes());
    for hash in &sorted_hashes {
        hasher.update(hash.as_slice());
    }

    // block_hashes already sorted (BTreeMap)
    hasher.update((block_hashes.len() as u64).to_be_bytes());
    for (num, hash) in &block_hashes {
        hasher.update(num.to_be_bytes::<32>());
        hasher.update(hash.as_slice());
    }

    B256::from_slice(&hasher.finalize())
}

/// Compute a deterministic SHA256 hash of EVM state accumulators.
pub fn hash_evm_state(evm_state: &EvmAccumulatorState) -> B256 {
    let mut hasher = Sha256::new();
    hasher.update(evm_state.cumulative_gas_used.to_be_bytes());
    hasher.update(evm_state.da_footprint_used.to_be_bytes());
    hasher.update(evm_state.blob_gas_used.to_be_bytes());
    hasher.update(<Bloom as AsRef<[u8]>>::as_ref(&evm_state.logs_bloom));
    // Encode receipts in execution order
    hasher.update((evm_state.receipts.len() as u64).to_be_bytes());
    for receipt in &evm_state.receipts {
        let encoded = alloy_rlp::encode(receipt);
        hasher.update((encoded.len() as u64).to_be_bytes());
        hasher.update(&encoded);
    }
    B256::from_slice(&hasher.finalize())
}

/// Compute the chunk_trace commitment from five input hashes.
/// Returns SHA256(tx_hash || pre_db_hash || post_db_hash || pre_evm_hash || post_evm_hash).
pub fn compute_chunk_trace(
    tx_hash: B256,
    pre_db_hash: B256,
    post_db_hash: B256,
    pre_evm_hash: B256,
    post_evm_hash: B256,
) -> B256 {
    let mut hasher = Sha256::new();
    hasher.update(tx_hash.as_slice());
    hasher.update(pre_db_hash.as_slice());
    hasher.update(post_db_hash.as_slice());
    hasher.update(pre_evm_hash.as_slice());
    hasher.update(post_evm_hash.as_slice());
    B256::from_slice(&hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_evm::revm::database::in_memory_db::AccountState;
    use alloy_evm::revm::database::states::{AccountStatus, CacheAccount};
    use alloy_evm::revm::state::Bytecode;
    use alloy_primitives::{address, Bloom, U256};
    use risc0_zkvm::sha::Digestible;

    fn make_info(nonce: u64, balance: u64) -> AccountInfo {
        AccountInfo {
            nonce,
            balance: U256::from(balance),
            code_hash: B256::ZERO,
            code: None,
        }
    }

    fn make_bytecode(bytes: &'static [u8]) -> Bytecode {
        Bytecode::new_raw(alloy_primitives::Bytes::from_static(bytes))
    }

    // ========== hash_canonical_state tests (2.3) ==========

    #[test]
    fn empty_state_has_deterministic_hash() {
        let cache = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        let h1 = hash_cache(&cache);
        let h2 = hash_cache(&cache);
        assert_eq!(h1, h2);
        assert!(!h1.is_zero());
    }

    #[test]
    fn cache_and_cache_state_same_logical_state_same_hash() {
        let addr = address!("0x1111111111111111111111111111111111111111");
        let info = make_info(1, 1000);

        // Build Cache
        let mut cache = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        cache.accounts.insert(
            addr,
            DbAccount {
                info: info.clone(),
                account_state: AccountState::None,
                storage: Default::default(),
            },
        );

        // Build CacheState with equivalent status
        let mut cache_state = CacheState::new(true);
        cache_state.accounts.insert(
            addr,
            CacheAccount {
                account: Some(alloy_evm::revm::database::states::PlainAccount {
                    info: info.clone(),
                    storage: Default::default(),
                }),
                status: AccountStatus::Loaded,
            },
        );

        let block_hashes = BTreeMap::new();
        let h_cache = hash_cache(&cache);
        let h_state = hash_cache_state(&cache_state, &block_hashes);
        assert_eq!(h_cache, h_state);
    }

    #[test]
    fn changed_status_maps_to_touched() {
        let addr = address!("0x2222222222222222222222222222222222222222");
        let info = make_info(5, 500);

        // Cache with Touched
        let mut cache = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        cache.accounts.insert(
            addr,
            DbAccount {
                info: info.clone(),
                account_state: AccountState::Touched,
                storage: Default::default(),
            },
        );

        // CacheState with Changed (maps to Touched)
        let mut cache_state = CacheState::new(true);
        cache_state.accounts.insert(
            addr,
            CacheAccount {
                account: Some(alloy_evm::revm::database::states::PlainAccount {
                    info: info.clone(),
                    storage: Default::default(),
                }),
                status: AccountStatus::Changed,
            },
        );

        let h_cache = hash_cache(&cache);
        let h_state = hash_cache_state(&cache_state, &BTreeMap::new());
        assert_eq!(h_cache, h_state);
    }

    #[test]
    fn in_memory_change_maps_to_touched() {
        let addr = address!("0x3333333333333333333333333333333333333333");
        let info = make_info(1, 100);

        let mut cache = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        cache.accounts.insert(
            addr,
            DbAccount {
                info: info.clone(),
                account_state: AccountState::Touched,
                storage: Default::default(),
            },
        );

        let mut cache_state = CacheState::new(true);
        cache_state.accounts.insert(
            addr,
            CacheAccount {
                account: Some(alloy_evm::revm::database::states::PlainAccount {
                    info: info.clone(),
                    storage: Default::default(),
                }),
                status: AccountStatus::InMemoryChange,
            },
        );

        assert_eq!(
            hash_cache(&cache),
            hash_cache_state(&cache_state, &BTreeMap::new())
        );
    }

    #[test]
    fn multiple_accounts_with_storage() {
        let addr1 = address!("0x1111111111111111111111111111111111111111");
        let addr2 = address!("0x2222222222222222222222222222222222222222");
        let info1 = make_info(1, 100);
        let info2 = make_info(2, 200);
        let slot = U256::from(42);
        let value = U256::from(999);

        let mut cache = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        cache.accounts.insert(
            addr1,
            DbAccount {
                info: info1.clone(),
                account_state: AccountState::Touched,
                storage: Default::default(),
            },
        );
        cache
            .accounts
            .get_mut(&addr1)
            .unwrap()
            .storage
            .insert(slot, value);
        cache.accounts.insert(
            addr2,
            DbAccount {
                info: info2.clone(),
                account_state: AccountState::None,
                storage: Default::default(),
            },
        );

        let mut cache_state = CacheState::new(true);
        cache_state.insert_account(addr1, info1.clone());
        cache_state.accounts.get_mut(&addr1).unwrap().status = AccountStatus::Changed;
        cache_state
            .accounts
            .get_mut(&addr1)
            .unwrap()
            .account
            .as_mut()
            .unwrap()
            .storage
            .insert(slot, value);
        cache_state.accounts.insert(
            addr2,
            CacheAccount {
                account: Some(alloy_evm::revm::database::states::PlainAccount {
                    info: info2.clone(),
                    storage: Default::default(),
                }),
                status: AccountStatus::Loaded,
            },
        );

        assert_eq!(
            hash_cache(&cache),
            hash_cache_state(&cache_state, &BTreeMap::new())
        );
    }

    #[test]
    fn contracts_included_in_hash() {
        let code = make_bytecode(&[0x60, 0x00]);
        let code_hash = code.hash_slow();

        let mut cache1 = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        let mut cache2 = cache1.clone();

        cache1.contracts.insert(code_hash, code.clone());

        let h1 = hash_cache(&cache1);
        let h2 = hash_cache(&cache2);
        assert_ne!(h1, h2);

        cache2.contracts.insert(code_hash, code);
        assert_eq!(hash_cache(&cache1), hash_cache(&cache2));
    }

    #[test]
    fn validate_cached_contracts_rejects_malformed_entry() {
        let valid_code = make_bytecode(&[0x60, 0x00]);
        let invalid_code = make_bytecode(&[0x60, 0x01]);
        let code_hash = valid_code.hash_slow();

        let mut cache = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        cache.contracts.insert(code_hash, invalid_code);

        let panic =
            std::panic::catch_unwind(|| validate_cached_contracts(&cache.contracts)).unwrap_err();
        let message = if let Some(msg) = panic.downcast_ref::<String>() {
            msg.clone()
        } else if let Some(msg) = panic.downcast_ref::<&str>() {
            msg.to_string()
        } else {
            String::new()
        };
        assert!(message.contains("cached contract bytecode hash mismatch"));
    }

    #[test]
    fn replacing_contract_with_different_validated_pair_changes_hash() {
        let code1 = make_bytecode(&[0x60, 0x00]);
        let code2 = make_bytecode(&[0x60, 0x01]);
        let hash1 = code1.hash_slow();
        let hash2 = code2.hash_slow();

        let mut cache1 = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        let mut cache2 = cache1.clone();
        cache1.contracts.insert(hash1, code1);
        cache2.contracts.insert(hash2, code2);

        assert_ne!(hash_cache(&cache1), hash_cache(&cache2));
    }

    #[test]
    fn block_hashes_included_in_hash() {
        let mut cache1 = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        let cache2 = cache1.clone();

        cache1
            .block_hashes
            .insert(U256::from(100), B256::repeat_byte(0xAA));

        let h1 = hash_cache(&cache1);
        let h2 = hash_cache(&cache2);
        assert_ne!(h1, h2);
    }

    #[test]
    fn nonce_change_produces_different_hash() {
        let addr = address!("0x1111111111111111111111111111111111111111");
        let mut cache = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        cache.accounts.insert(
            addr,
            DbAccount {
                info: make_info(1, 100),
                account_state: AccountState::None,
                storage: Default::default(),
            },
        );
        let h1 = hash_cache(&cache);

        cache.accounts.get_mut(&addr).unwrap().info.nonce = 2;
        let h2 = hash_cache(&cache);
        assert_ne!(h1, h2);
    }

    #[test]
    fn balance_change_produces_different_hash() {
        let addr = address!("0x1111111111111111111111111111111111111111");
        let mut cache = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        cache.accounts.insert(
            addr,
            DbAccount {
                info: make_info(1, 100),
                account_state: AccountState::None,
                storage: Default::default(),
            },
        );
        let h1 = hash_cache(&cache);

        cache.accounts.get_mut(&addr).unwrap().info.balance = U256::from(200);
        let h2 = hash_cache(&cache);
        assert_ne!(h1, h2);
    }

    #[test]
    fn storage_slot_change_produces_different_hash() {
        let addr = address!("0x1111111111111111111111111111111111111111");
        let mut cache = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        cache.accounts.insert(
            addr,
            DbAccount {
                info: make_info(1, 100),
                account_state: AccountState::None,
                storage: Default::default(),
            },
        );
        cache
            .accounts
            .get_mut(&addr)
            .unwrap()
            .storage
            .insert(U256::from(1), U256::from(10));
        let h1 = hash_cache(&cache);

        cache
            .accounts
            .get_mut(&addr)
            .unwrap()
            .storage
            .insert(U256::from(1), U256::from(20));
        let h2 = hash_cache(&cache);
        assert_ne!(h1, h2);
    }

    #[test]
    fn account_state_change_produces_different_hash() {
        let addr = address!("0x1111111111111111111111111111111111111111");
        let mut cache = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        cache.accounts.insert(
            addr,
            DbAccount {
                info: make_info(1, 100),
                account_state: AccountState::None,
                storage: Default::default(),
            },
        );
        let h1 = hash_cache(&cache);

        cache.accounts.get_mut(&addr).unwrap().account_state = AccountState::Touched;
        let h2 = hash_cache(&cache);
        assert_ne!(h1, h2);
    }

    #[test]
    fn insertion_order_independence() {
        let addr1 = address!("0x1111111111111111111111111111111111111111");
        let addr2 = address!("0x2222222222222222222222222222222222222222");
        let info1 = make_info(1, 100);
        let info2 = make_info(2, 200);

        let mut cache1 = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        // Insert addr1 then addr2
        cache1.accounts.insert(
            addr1,
            DbAccount {
                info: info1.clone(),
                account_state: AccountState::None,
                storage: Default::default(),
            },
        );
        cache1.accounts.insert(
            addr2,
            DbAccount {
                info: info2.clone(),
                account_state: AccountState::None,
                storage: Default::default(),
            },
        );

        let mut cache2 = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        // Insert addr2 then addr1
        cache2.accounts.insert(
            addr2,
            DbAccount {
                info: info2,
                account_state: AccountState::None,
                storage: Default::default(),
            },
        );
        cache2.accounts.insert(
            addr1,
            DbAccount {
                info: info1,
                account_state: AccountState::None,
                storage: Default::default(),
            },
        );

        assert_eq!(hash_cache(&cache1), hash_cache(&cache2));
    }

    #[test]
    fn log_only_changes_do_not_affect_hash() {
        let cache1 = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        let mut cache2 = cache1.clone();

        // Add logs to cache2 only
        cache2.logs.push(alloy_primitives::Log::new_unchecked(
            Address::ZERO,
            vec![],
            alloy_primitives::Bytes::new(),
        ));

        assert_eq!(hash_cache(&cache1), hash_cache(&cache2));
    }

    #[test]
    fn overlay_preserves_untouched_base_entries() {
        let addr_base = address!("0x1111111111111111111111111111111111111111");
        let addr_live = address!("0x2222222222222222222222222222222222222222");

        // Base cache with addr_base
        let mut base = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        base.accounts.insert(
            addr_base,
            DbAccount {
                info: make_info(1, 100),
                account_state: AccountState::None,
                storage: Default::default(),
            },
        );

        // Live CacheState only touched addr_live
        let mut live = CacheState::new(true);
        live.accounts.insert(
            addr_live,
            CacheAccount {
                account: Some(alloy_evm::revm::database::states::PlainAccount {
                    info: make_info(2, 200),
                    storage: Default::default(),
                }),
                status: AccountStatus::Changed,
            },
        );

        let overlay_hash = hash_overlay_state(&base, &live, &BTreeMap::new());

        // Build equivalent full Cache manually
        let mut full_cache = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        full_cache.accounts.insert(
            addr_base,
            DbAccount {
                info: make_info(1, 100),
                account_state: AccountState::None,
                storage: Default::default(),
            },
        );
        full_cache.accounts.insert(
            addr_live,
            DbAccount {
                info: make_info(2, 200),
                account_state: AccountState::Touched,
                storage: Default::default(),
            },
        );

        assert_eq!(overlay_hash, hash_cache(&full_cache));
    }

    #[test]
    fn overlay_storage_merge() {
        let addr = address!("0x1111111111111111111111111111111111111111");
        let info = make_info(1, 100);

        // Base has slot 1 = 10, slot 2 = 20
        let mut base = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        base.accounts.insert(
            addr,
            DbAccount {
                info: info.clone(),
                account_state: AccountState::None,
                storage: Default::default(),
            },
        );
        let base_storage = &mut base.accounts.get_mut(&addr).unwrap().storage;
        base_storage.insert(U256::from(1), U256::from(10));
        base_storage.insert(U256::from(2), U256::from(20));

        // Live overwrites slot 1 = 99, doesn't touch slot 2
        let mut live = CacheState::new(true);
        live.insert_account(addr, info.clone());
        live.accounts.get_mut(&addr).unwrap().status = AccountStatus::Changed;
        live.accounts
            .get_mut(&addr)
            .unwrap()
            .account
            .as_mut()
            .unwrap()
            .storage
            .insert(U256::from(1), U256::from(99));

        let overlay_hash = hash_overlay_state(&base, &live, &BTreeMap::new());

        // Equivalent full cache: slot 1 = 99, slot 2 = 20
        let mut full = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        full.accounts.insert(
            addr,
            DbAccount {
                info,
                account_state: AccountState::Touched,
                storage: Default::default(),
            },
        );
        let full_storage = &mut full.accounts.get_mut(&addr).unwrap().storage;
        full_storage.insert(U256::from(1), U256::from(99));
        full_storage.insert(U256::from(2), U256::from(20));

        assert_eq!(overlay_hash, hash_cache(&full));
    }

    #[test]
    fn overlay_destroyed_account_drops_inherited_base_info_and_storage() {
        let addr = address!("0x3333333333333333333333333333333333333333");

        let mut base = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        base.accounts.insert(
            addr,
            DbAccount {
                info: make_info(7, 700),
                account_state: AccountState::Touched,
                storage: HashMap::from_iter([
                    (U256::from(1), U256::from(10)),
                    (U256::from(2), U256::from(20)),
                ]),
            },
        );

        let mut live = CacheState::new(true);
        live.accounts.insert(addr, CacheAccount::new_destroyed());

        let overlay_hash = hash_overlay_state(&base, &live, &BTreeMap::new());

        let mut expected = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        expected.accounts.insert(
            addr,
            DbAccount {
                info: AccountInfo::default(),
                account_state: AccountState::StorageCleared,
                storage: Default::default(),
            },
        );

        assert_eq!(overlay_hash, hash_cache(&expected));
    }

    #[test]
    fn overlay_destroyed_changed_drops_inherited_base_storage() {
        let addr = address!("0x4444444444444444444444444444444444444444");
        let recreated_info = make_info(9, 900);

        let mut base = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        base.accounts.insert(
            addr,
            DbAccount {
                info: make_info(1, 100),
                account_state: AccountState::Touched,
                storage: HashMap::from_iter([
                    (U256::from(1), U256::from(10)),
                    (U256::from(2), U256::from(20)),
                ]),
            },
        );

        let mut live = CacheState::new(true);
        live.accounts.insert(
            addr,
            CacheAccount {
                account: Some(alloy_evm::revm::database::states::PlainAccount {
                    info: recreated_info.clone(),
                    storage: HashMap::from_iter([(U256::from(3), U256::from(30))]),
                }),
                status: AccountStatus::DestroyedChanged,
            },
        );

        let overlay_hash = hash_overlay_state(&base, &live, &BTreeMap::new());

        let mut expected = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        expected.accounts.insert(
            addr,
            DbAccount {
                info: recreated_info,
                account_state: AccountState::StorageCleared,
                storage: HashMap::from_iter([(U256::from(3), U256::from(30))]),
            },
        );

        assert_eq!(overlay_hash, hash_cache(&expected));
    }

    #[test]
    fn overlay_loaded_not_existing_drops_inherited_base_account() {
        let addr = address!("0x5555555555555555555555555555555555555555");

        let mut base = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        base.accounts.insert(
            addr,
            DbAccount {
                info: make_info(3, 300),
                account_state: AccountState::None,
                storage: HashMap::from_iter([(U256::from(1), U256::from(10))]),
            },
        );

        let mut live = CacheState::new(true);
        live.accounts
            .insert(addr, CacheAccount::new_loaded_not_existing());

        let overlay_hash = hash_overlay_state(&base, &live, &BTreeMap::new());

        let mut expected = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        expected
            .accounts
            .insert(addr, DbAccount::new_not_existing());

        assert_eq!(overlay_hash, hash_cache(&expected));
    }

    // ========== hash_evm_state tests (2.5) ==========

    #[test]
    fn evm_state_zeroed_has_deterministic_hash() {
        let state = EvmAccumulatorState::default();
        let h1 = hash_evm_state(&state);
        let h2 = hash_evm_state(&state);
        assert_eq!(h1, h2);
        assert!(!h1.is_zero());
    }

    #[test]
    fn evm_state_gas_change_different_hash() {
        let s1 = EvmAccumulatorState {
            cumulative_gas_used: 100,
            ..Default::default()
        };
        let s2 = EvmAccumulatorState::default();
        assert_ne!(hash_evm_state(&s1), hash_evm_state(&s2));
    }

    #[test]
    fn evm_state_da_footprint_change_different_hash() {
        let s1 = EvmAccumulatorState {
            da_footprint_used: 50,
            ..Default::default()
        };
        let s2 = EvmAccumulatorState::default();
        assert_ne!(hash_evm_state(&s1), hash_evm_state(&s2));
    }

    #[test]
    fn evm_state_blob_gas_change_different_hash() {
        let s1 = EvmAccumulatorState {
            blob_gas_used: 75,
            ..Default::default()
        };
        let s2 = EvmAccumulatorState::default();
        assert_ne!(hash_evm_state(&s1), hash_evm_state(&s2));
    }

    #[test]
    fn evm_state_bloom_change_different_hash() {
        let s1 = EvmAccumulatorState {
            logs_bloom: Bloom::repeat_byte(0xFF),
            ..Default::default()
        };
        let s2 = EvmAccumulatorState::default();
        assert_ne!(hash_evm_state(&s1), hash_evm_state(&s2));
    }

    #[test]
    fn evm_state_receipt_ordering_matters() {
        use op_alloy_consensus::OpTxType;

        let empty_logs: Vec<alloy_primitives::Log> = vec![];
        let r1 =
            OpReceiptEnvelope::from_parts(true, 100, &empty_logs, OpTxType::Legacy, None, None);
        let r2 =
            OpReceiptEnvelope::from_parts(true, 200, &empty_logs, OpTxType::Legacy, None, None);

        let s1 = EvmAccumulatorState {
            receipts: vec![r1.clone(), r2.clone()],
            ..Default::default()
        };
        let s2 = EvmAccumulatorState {
            receipts: vec![r2, r1],
            ..Default::default()
        };

        assert_ne!(hash_evm_state(&s1), hash_evm_state(&s2));
    }

    // ========== compute_chunk_trace tests (2.7) ==========

    #[test]
    fn chunk_trace_deterministic() {
        let a = B256::repeat_byte(0x01);
        let b = B256::repeat_byte(0x02);
        let c = B256::repeat_byte(0x03);
        let d = B256::repeat_byte(0x04);
        let e = B256::repeat_byte(0x05);

        let t1 = compute_chunk_trace(a, b, c, d, e);
        let t2 = compute_chunk_trace(a, b, c, d, e);
        assert_eq!(t1, t2);
        assert!(!t1.is_zero());
    }

    #[test]
    fn chunk_trace_any_input_change_different() {
        let base = [
            B256::repeat_byte(0x01),
            B256::repeat_byte(0x02),
            B256::repeat_byte(0x03),
            B256::repeat_byte(0x04),
            B256::repeat_byte(0x05),
        ];
        let baseline = compute_chunk_trace(base[0], base[1], base[2], base[3], base[4]);

        for i in 0..5 {
            let mut modified = base;
            modified[i] = B256::repeat_byte(0xFF);
            let h = compute_chunk_trace(
                modified[0],
                modified[1],
                modified[2],
                modified[3],
                modified[4],
            );
            assert_ne!(
                baseline, h,
                "changing input {i} should produce different trace"
            );
        }
    }

    #[test]
    fn chunk_trace_integration_with_precondition() {
        let trace = compute_chunk_trace(
            B256::repeat_byte(0x01),
            B256::repeat_byte(0x02),
            B256::repeat_byte(0x03),
            B256::repeat_byte(0x04),
            B256::repeat_byte(0x05),
        );
        let p = crate::precondition::Precondition::default().chunk(trace);
        assert_eq!(p.digest(), risc0_zkvm::Digest::from_bytes(trace.0));
    }
}

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

//! rkyv `ArchiveWith` wrappers for revm's `Cache` and `EvmAccumulatorState`.
//!
//! revm's `Cache`, `DbAccount`, `AccountInfo`, and `Bytecode` have no rkyv support.
//! This module bridges the gap using the `ArchiveWith` / `SerializeWith` / `DeserializeWith`
//! pattern consistent with `execution.rs` and other rkyv modules. Fields are decomposed into
//! tuples of rkyv-native types and rkyv handles the actual serialization.

use alloy_evm::revm::database::in_memory_db::{AccountState, Cache, DbAccount};
use alloy_evm::revm::state::{AccountInfo, Bytecode};
use alloy_primitives::{Address, Bloom, Bytes, B256, U256};
use rkyv::rancor::Fallible;
use rkyv::with::{ArchiveWith, DeserializeWith, SerializeWith};
use rkyv::{Archive, Archived, Place, Resolver};

use crate::precondition::chunking::{account_state_byte, EvmAccumulatorState};

fn account_state_from_byte(byte: u8) -> AccountState {
    match byte {
        0 => AccountState::NotExisting,
        1 => AccountState::None,
        2 => AccountState::Touched,
        3 => AccountState::StorageCleared,
        _ => panic!("invalid AccountState byte: {byte}"),
    }
}

/// (address, nonce, balance, code_hash, code, account_state_byte, sorted_storage)
type RkyvedAccount = (
    [u8; 20],
    u64,
    [u8; 32],
    [u8; 32],
    Option<Vec<u8>>,
    u8,
    Vec<([u8; 32], [u8; 32])>,
);

/// (address, topics, data)
type RkyvedLog = ([u8; 20], Vec<[u8; 32]>, Vec<u8>);

/// (sorted_accounts, sorted_contracts, logs, sorted_block_hashes)
pub type RkyvedCache = (
    Vec<RkyvedAccount>,
    Vec<([u8; 32], Vec<u8>)>,
    Vec<RkyvedLog>,
    Vec<([u8; 32], [u8; 32])>,
);

/// rkyv wrapper for revm's `Cache`.
///
/// Archives `Cache` as a tuple of rkyv-native types via [`RkyvedCache`].
pub struct CacheRkyv;

impl CacheRkyv {
    pub fn rkyv(cache: &Cache) -> RkyvedCache {
        // Accounts (sorted by address)
        let mut accounts: Vec<_> = cache.accounts.iter().collect();
        accounts.sort_by_key(|(addr, _)| *addr);
        let accounts = accounts
            .into_iter()
            .map(|(addr, acct)| {
                let mut storage: Vec<_> = acct.storage.iter().collect();
                storage.sort_by_key(|(k, _)| *k);
                let storage = storage
                    .into_iter()
                    .map(|(k, v)| (k.to_be_bytes::<32>(), v.to_be_bytes::<32>()))
                    .collect();
                (
                    *addr.0,
                    acct.info.nonce,
                    acct.info.balance.to_be_bytes::<32>(),
                    acct.info.code_hash.0,
                    acct.info.code.as_ref().map(|c| c.original_bytes().to_vec()),
                    account_state_byte(&acct.account_state),
                    storage,
                )
            })
            .collect();

        // Contracts (sorted by code_hash)
        let mut contracts: Vec<_> = cache.contracts.iter().collect();
        contracts.sort_by_key(|(hash, _)| *hash);
        let contracts = contracts
            .into_iter()
            .map(|(hash, bytecode)| (hash.0, bytecode.original_bytes().to_vec()))
            .collect();

        // Logs
        let logs = cache
            .logs
            .iter()
            .map(|log| {
                (
                    *log.address.0,
                    log.data.topics().iter().map(|t| t.0).collect(),
                    log.data.data.to_vec(),
                )
            })
            .collect();

        // Block hashes (sorted by block number)
        let mut block_hashes: Vec<_> = cache.block_hashes.iter().collect();
        block_hashes.sort_by_key(|(num, _)| *num);
        let block_hashes = block_hashes
            .into_iter()
            .map(|(num, hash)| (num.to_be_bytes::<32>(), hash.0))
            .collect();

        (accounts, contracts, logs, block_hashes)
    }

    pub fn raw(rkyved: RkyvedCache) -> Cache {
        let (accounts, contracts, logs, block_hashes) = rkyved;

        let accounts = accounts
            .into_iter()
            .map(|(addr, nonce, balance, code_hash, code, state, storage)| {
                let storage = storage
                    .into_iter()
                    .map(|(k, v)| (U256::from_be_bytes(k), U256::from_be_bytes(v)))
                    .collect();
                (
                    Address::from(addr),
                    DbAccount {
                        info: AccountInfo {
                            nonce,
                            balance: U256::from_be_bytes(balance),
                            code_hash: B256::new(code_hash),
                            code: code.map(|raw| Bytecode::new_raw(Bytes::from(raw))),
                        },
                        account_state: account_state_from_byte(state),
                        storage,
                    },
                )
            })
            .collect();

        let contracts = contracts
            .into_iter()
            .map(|(hash, raw)| (B256::new(hash), Bytecode::new_raw(Bytes::from(raw))))
            .collect();

        let logs = logs
            .into_iter()
            .map(|(addr, topics, data)| {
                alloy_primitives::Log::new_unchecked(
                    Address::from(addr),
                    topics.into_iter().map(B256::new).collect(),
                    Bytes::from(data),
                )
            })
            .collect();

        let block_hashes = block_hashes
            .into_iter()
            .map(|(num, hash)| (U256::from_be_bytes(num), B256::new(hash)))
            .collect();

        Cache {
            accounts,
            contracts,
            logs,
            block_hashes,
        }
    }
}

impl ArchiveWith<Cache> for CacheRkyv {
    type Archived = Archived<RkyvedCache>;
    type Resolver = Resolver<RkyvedCache>;

    fn resolve_with(field: &Cache, resolver: Self::Resolver, out: Place<Self::Archived>) {
        let rkyved = CacheRkyv::rkyv(field);
        <RkyvedCache as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<Cache, S> for CacheRkyv
where
    S: Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    <S as Fallible>::Error: rkyv::rancor::Source,
{
    fn serialize_with(field: &Cache, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        let rkyved = CacheRkyv::rkyv(field);
        <RkyvedCache as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedCache>, Cache, D> for CacheRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: rkyv::rancor::Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedCache>,
        deserializer: &mut D,
    ) -> Result<Cache, D::Error> {
        let rkyved: RkyvedCache = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(CacheRkyv::raw(rkyved))
    }
}

// -- EvmAccumulatorStateRkyv --

/// (cumulative_gas_used, da_footprint_used, blob_gas_used, logs_bloom, rlp_receipts)
pub type RkyvedEvmAccumulatorState = (u64, u64, u64, [u8; 256], Vec<u8>);

/// rkyv wrapper for [`EvmAccumulatorState`].
///
/// Archives as a tuple of rkyv-native types via [`RkyvedEvmAccumulatorState`].
/// Receipts are RLP-encoded, matching the pattern in `execution.rs`.
pub struct EvmAccumulatorStateRkyv;

impl EvmAccumulatorStateRkyv {
    pub fn rkyv(state: &EvmAccumulatorState) -> RkyvedEvmAccumulatorState {
        (
            state.cumulative_gas_used,
            state.da_footprint_used,
            state.blob_gas_used,
            *state.logs_bloom.0,
            alloy_rlp::encode(&state.receipts),
        )
    }

    pub fn raw(rkyved: RkyvedEvmAccumulatorState) -> EvmAccumulatorState {
        EvmAccumulatorState {
            cumulative_gas_used: rkyved.0,
            da_footprint_used: rkyved.1,
            blob_gas_used: rkyved.2,
            logs_bloom: Bloom::new(rkyved.3),
            receipts: alloy_rlp::decode_exact(rkyved.4.as_slice()).unwrap(),
        }
    }
}

impl ArchiveWith<EvmAccumulatorState> for EvmAccumulatorStateRkyv {
    type Archived = Archived<RkyvedEvmAccumulatorState>;
    type Resolver = Resolver<RkyvedEvmAccumulatorState>;

    fn resolve_with(
        field: &EvmAccumulatorState,
        resolver: Self::Resolver,
        out: Place<Self::Archived>,
    ) {
        let rkyved = EvmAccumulatorStateRkyv::rkyv(field);
        <RkyvedEvmAccumulatorState as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<EvmAccumulatorState, S> for EvmAccumulatorStateRkyv
where
    S: Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    <S as Fallible>::Error: rkyv::rancor::Source,
{
    fn serialize_with(
        field: &EvmAccumulatorState,
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        let rkyved = EvmAccumulatorStateRkyv::rkyv(field);
        <RkyvedEvmAccumulatorState as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedEvmAccumulatorState>, EvmAccumulatorState, D>
    for EvmAccumulatorStateRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: rkyv::rancor::Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedEvmAccumulatorState>,
        deserializer: &mut D,
    ) -> Result<EvmAccumulatorState, D::Error> {
        let rkyved: RkyvedEvmAccumulatorState =
            rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(EvmAccumulatorStateRkyv::raw(rkyved))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{from_bytes_with, to_bytes_with};
    use alloy_primitives::address;

    fn make_info(nonce: u64, balance: u64) -> AccountInfo {
        AccountInfo {
            nonce,
            balance: U256::from(balance),
            code_hash: B256::ZERO,
            code: None,
        }
    }

    #[test]
    fn empty_cache_round_trip() {
        let cache = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        let bytes = to_bytes_with!(CacheRkyv, &cache);
        let deser = from_bytes_with!(CacheRkyv, Cache, &bytes);
        assert!(deser.accounts.is_empty());
        assert!(deser.contracts.is_empty());
        assert!(deser.logs.is_empty());
        assert!(deser.block_hashes.is_empty());
    }

    #[test]
    fn cache_with_accounts_storage_contracts_block_hashes() {
        let addr1 = address!("0x1111111111111111111111111111111111111111");
        let addr2 = address!("0x2222222222222222222222222222222222222222");
        let code = Bytecode::new_raw(Bytes::from_static(&[0x60, 0x00, 0x60, 0x00]));
        let code_hash = code.hash_slow();

        let mut cache = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        cache.accounts.insert(
            addr1,
            DbAccount {
                info: make_info(1, 100),
                account_state: AccountState::Touched,
                storage: Default::default(),
            },
        );
        cache
            .accounts
            .get_mut(&addr1)
            .unwrap()
            .storage
            .insert(U256::from(42), U256::from(999));
        cache.accounts.insert(
            addr2,
            DbAccount {
                info: make_info(2, 200),
                account_state: AccountState::None,
                storage: Default::default(),
            },
        );
        cache.contracts.insert(code_hash, code.clone());
        cache
            .block_hashes
            .insert(U256::from(100), B256::repeat_byte(0xCC));

        let bytes = to_bytes_with!(CacheRkyv, &cache);
        let deser = from_bytes_with!(CacheRkyv, Cache, &bytes);

        assert_eq!(deser.accounts.len(), 2);
        assert_eq!(deser.accounts.get(&addr1).unwrap().info.nonce, 1);
        assert_eq!(
            deser.accounts.get(&addr1).unwrap().info.balance,
            U256::from(100)
        );
        assert_eq!(
            deser
                .accounts
                .get(&addr1)
                .unwrap()
                .storage
                .get(&U256::from(42)),
            Some(&U256::from(999))
        );
        assert_eq!(deser.accounts.get(&addr2).unwrap().info.nonce, 2);
        assert_eq!(
            deser.accounts.get(&addr2).unwrap().account_state,
            AccountState::None
        );
        assert_eq!(deser.contracts.len(), 1);
        assert_eq!(
            deser.contracts.get(&code_hash).unwrap().original_bytes(),
            code.original_bytes()
        );
        assert_eq!(deser.block_hashes.len(), 1);
        assert_eq!(
            deser.block_hashes.get(&U256::from(100)),
            Some(&B256::repeat_byte(0xCC))
        );
    }

    #[test]
    fn logs_preserved_on_round_trip() {
        let mut cache = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        cache.logs.push(alloy_primitives::Log::new_unchecked(
            Address::ZERO,
            vec![B256::repeat_byte(0xAA)],
            Bytes::from_static(&[0x01, 0x02, 0x03]),
        ));
        cache.logs.push(alloy_primitives::Log::new_unchecked(
            address!("0x1111111111111111111111111111111111111111"),
            vec![B256::repeat_byte(0xBB), B256::repeat_byte(0xCC)],
            Bytes::new(),
        ));
        assert_eq!(cache.logs.len(), 2);

        let bytes = to_bytes_with!(CacheRkyv, &cache);
        let deser = from_bytes_with!(CacheRkyv, Cache, &bytes);
        assert_eq!(deser.logs.len(), 2);
        assert_eq!(deser.logs[0].address, Address::ZERO);
        assert_eq!(deser.logs[0].topics(), &[B256::repeat_byte(0xAA)]);
        assert_eq!(deser.logs[0].data.data.as_ref(), &[0x01, 0x02, 0x03]);
        assert_eq!(
            deser.logs[1].address,
            address!("0x1111111111111111111111111111111111111111")
        );
        assert_eq!(
            deser.logs[1].topics(),
            &[B256::repeat_byte(0xBB), B256::repeat_byte(0xCC)]
        );
        assert!(deser.logs[1].data.data.is_empty());
    }

    #[test]
    fn account_code_round_trip() {
        let addr = address!("0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        let code = Bytecode::new_raw(Bytes::from_static(&[0x60, 0x00, 0x60, 0x00]));
        let mut cache = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        cache.accounts.insert(
            addr,
            DbAccount {
                info: AccountInfo {
                    nonce: 1,
                    balance: U256::from(100),
                    code_hash: code.hash_slow(),
                    code: Some(code.clone()),
                },
                account_state: AccountState::Touched,
                storage: Default::default(),
            },
        );
        let bytes = to_bytes_with!(CacheRkyv, &cache);
        let deser = from_bytes_with!(CacheRkyv, Cache, &bytes);
        let acct = deser.accounts.get(&addr).unwrap();
        assert_eq!(
            acct.info.code.as_ref().unwrap().original_bytes(),
            code.original_bytes()
        );

        // Also verify None round-trips
        cache.accounts.get_mut(&addr).unwrap().info.code = None;
        let bytes = to_bytes_with!(CacheRkyv, &cache);
        let deser = from_bytes_with!(CacheRkyv, Cache, &bytes);
        assert!(deser.accounts.get(&addr).unwrap().info.code.is_none());
    }

    #[test]
    fn empty_evm_accumulator_round_trip() {
        let state = EvmAccumulatorState::default();
        let bytes = to_bytes_with!(EvmAccumulatorStateRkyv, &state);
        let deser = from_bytes_with!(EvmAccumulatorStateRkyv, EvmAccumulatorState, &bytes);
        assert_eq!(deser, state);
    }

    #[test]
    fn evm_accumulator_with_receipts_round_trip() {
        use op_alloy_consensus::{OpReceiptEnvelope, OpTxType};

        let state = EvmAccumulatorState {
            cumulative_gas_used: 21000,
            da_footprint_used: 500,
            blob_gas_used: 131072,
            logs_bloom: Bloom::repeat_byte(0x42),
            receipts: vec![
                OpReceiptEnvelope::from_parts(true, 21000, vec![], OpTxType::Legacy, None, None),
                OpReceiptEnvelope::from_parts(false, 42000, vec![], OpTxType::Eip1559, None, None),
            ],
        };
        let bytes = to_bytes_with!(EvmAccumulatorStateRkyv, &state);
        let deser = from_bytes_with!(EvmAccumulatorStateRkyv, EvmAccumulatorState, &bytes);
        assert_eq!(deser.cumulative_gas_used, 21000);
        assert_eq!(deser.da_footprint_used, 500);
        assert_eq!(deser.blob_gas_used, 131072);
        assert_eq!(deser.logs_bloom, Bloom::repeat_byte(0x42));
        assert_eq!(deser.receipts.len(), 2);
        assert_eq!(deser.receipts, state.receipts);
    }

    #[test]
    fn all_account_states_round_trip() {
        let addr = address!("0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        for state in [
            AccountState::NotExisting,
            AccountState::Touched,
            AccountState::StorageCleared,
            AccountState::None,
        ] {
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
                    account_state: state.clone(),
                    storage: Default::default(),
                },
            );
            let bytes = to_bytes_with!(CacheRkyv, &cache);
            let deser = from_bytes_with!(CacheRkyv, Cache, &bytes);
            assert_eq!(deser.accounts.get(&addr).unwrap().account_state, state);
        }
    }
}

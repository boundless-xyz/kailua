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

use alloy_evm::revm::context::BlockEnv;
use alloy_evm::revm::context_interface::block::BlobExcessGasAndPrice;
use alloy_evm::revm::database::in_memory_db::{AccountState, Cache, DbAccount};
use alloy_evm::revm::state::{AccountInfo, Bytecode};
use alloy_op_evm::block::OpBlockExecutionCtx;
use alloy_primitives::{Address, Bytes, B256, U256};
use op_alloy_consensus::OpReceiptEnvelope;
use rkyv::rancor::Fallible;
use rkyv::with::{ArchiveWith, DeserializeWith, SerializeWith};
use rkyv::{Archive, Archived, Place, Resolver};

use crate::precondition::chunking::account_state_byte;

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

// -- OpReceiptRlpRkyv --

/// rkyv wrapper for a single [`OpReceiptEnvelope`] using RLP encoding.
///
/// Compose with `rkyv::with::Map<OpReceiptRlpRkyv>` to serialize `Vec<OpReceiptEnvelope>`.
pub struct OpReceiptRlpRkyv;

impl ArchiveWith<OpReceiptEnvelope> for OpReceiptRlpRkyv {
    type Archived = Archived<Vec<u8>>;
    type Resolver = Resolver<Vec<u8>>;

    fn resolve_with(
        field: &OpReceiptEnvelope,
        resolver: Self::Resolver,
        out: Place<Self::Archived>,
    ) {
        let encoded = alloy_rlp::encode(field);
        <Vec<u8> as Archive>::resolve(&encoded, resolver, out);
    }
}

impl<S> SerializeWith<OpReceiptEnvelope, S> for OpReceiptRlpRkyv
where
    S: Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    <S as Fallible>::Error: rkyv::rancor::Source,
{
    fn serialize_with(
        field: &OpReceiptEnvelope,
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        let encoded = alloy_rlp::encode(field);
        <Vec<u8> as rkyv::Serialize<S>>::serialize(&encoded, serializer)
    }
}

impl<D> DeserializeWith<Archived<Vec<u8>>, OpReceiptEnvelope, D> for OpReceiptRlpRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: rkyv::rancor::Source,
{
    fn deserialize_with(
        field: &Archived<Vec<u8>>,
        deserializer: &mut D,
    ) -> Result<OpReceiptEnvelope, D::Error> {
        let bytes: Vec<u8> = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(alloy_rlp::decode_exact(bytes.as_slice()).unwrap())
    }
}

// -- BlockEnvRkyv --

/// (number, beneficiary, timestamp, gas_limit, basefee, difficulty, prevrandao, blob_excess_gas_and_price)
type RkyvedBlockEnv = (
    U256,
    Address,
    U256,
    u64,
    u64,
    U256,
    Option<B256>,
    Option<(u64, u128)>,
);

/// rkyv wrapper for revm's [`BlockEnv`].
///
/// Archives as a tuple of rkyv-native types via [`RkyvedBlockEnv`].
/// `BlobExcessGasAndPrice` is decomposed to `(u64, u128)` since it lacks rkyv support.
pub struct BlockEnvRkyv;

impl BlockEnvRkyv {
    pub fn rkyv(env: &BlockEnv) -> RkyvedBlockEnv {
        (
            env.number,
            env.beneficiary,
            env.timestamp,
            env.gas_limit,
            env.basefee,
            env.difficulty,
            env.prevrandao,
            env.blob_excess_gas_and_price
                .as_ref()
                .map(|b| (b.excess_blob_gas, b.blob_gasprice)),
        )
    }

    pub fn raw(r: RkyvedBlockEnv) -> BlockEnv {
        BlockEnv {
            number: r.0,
            beneficiary: r.1,
            timestamp: r.2,
            gas_limit: r.3,
            basefee: r.4,
            difficulty: r.5,
            prevrandao: r.6,
            blob_excess_gas_and_price: r.7.map(|(excess, price)| BlobExcessGasAndPrice {
                excess_blob_gas: excess,
                blob_gasprice: price,
            }),
        }
    }
}

impl ArchiveWith<BlockEnv> for BlockEnvRkyv {
    type Archived = Archived<RkyvedBlockEnv>;
    type Resolver = Resolver<RkyvedBlockEnv>;

    fn resolve_with(field: &BlockEnv, resolver: Self::Resolver, out: Place<Self::Archived>) {
        let rkyved = BlockEnvRkyv::rkyv(field);
        <RkyvedBlockEnv as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<BlockEnv, S> for BlockEnvRkyv
where
    S: Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    <S as Fallible>::Error: rkyv::rancor::Source,
{
    fn serialize_with(field: &BlockEnv, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        let rkyved = BlockEnvRkyv::rkyv(field);
        <RkyvedBlockEnv as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedBlockEnv>, BlockEnv, D> for BlockEnvRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: rkyv::rancor::Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedBlockEnv>,
        deserializer: &mut D,
    ) -> Result<BlockEnv, D::Error> {
        let rkyved: RkyvedBlockEnv = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(BlockEnvRkyv::raw(rkyved))
    }
}

// -- OpBlockExecutionCtxRkyv --

/// (parent_hash, parent_beacon_block_root, extra_data)
type RkyvedOpBlockExecutionCtx = (B256, Option<B256>, Bytes);

/// rkyv wrapper for [`OpBlockExecutionCtx`].
///
/// Archives as a tuple of rkyv-native types via [`RkyvedOpBlockExecutionCtx`].
/// All fields have native rkyv support via the `alloy-primitives` `rkyv` feature.
pub struct OpBlockExecutionCtxRkyv;

impl OpBlockExecutionCtxRkyv {
    pub fn rkyv(ctx: &OpBlockExecutionCtx) -> RkyvedOpBlockExecutionCtx {
        (
            ctx.parent_hash,
            ctx.parent_beacon_block_root,
            ctx.extra_data.clone(),
        )
    }

    pub fn raw(r: RkyvedOpBlockExecutionCtx) -> OpBlockExecutionCtx {
        OpBlockExecutionCtx {
            parent_hash: r.0,
            parent_beacon_block_root: r.1,
            extra_data: r.2,
        }
    }
}

impl ArchiveWith<OpBlockExecutionCtx> for OpBlockExecutionCtxRkyv {
    type Archived = Archived<RkyvedOpBlockExecutionCtx>;
    type Resolver = Resolver<RkyvedOpBlockExecutionCtx>;

    fn resolve_with(
        field: &OpBlockExecutionCtx,
        resolver: Self::Resolver,
        out: Place<Self::Archived>,
    ) {
        let rkyved = OpBlockExecutionCtxRkyv::rkyv(field);
        <RkyvedOpBlockExecutionCtx as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<OpBlockExecutionCtx, S> for OpBlockExecutionCtxRkyv
where
    S: Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    <S as Fallible>::Error: rkyv::rancor::Source,
{
    fn serialize_with(
        field: &OpBlockExecutionCtx,
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        let rkyved = OpBlockExecutionCtxRkyv::rkyv(field);
        <RkyvedOpBlockExecutionCtx as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedOpBlockExecutionCtx>, OpBlockExecutionCtx, D>
    for OpBlockExecutionCtxRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: rkyv::rancor::Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedOpBlockExecutionCtx>,
        deserializer: &mut D,
    ) -> Result<OpBlockExecutionCtx, D::Error> {
        let rkyved: RkyvedOpBlockExecutionCtx =
            rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(OpBlockExecutionCtxRkyv::raw(rkyved))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{from_bytes_with, to_bytes_with};
    use alloy_primitives::{address, Bloom};

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
        use crate::precondition::chunking::EvmAccumulatorState;
        let state = EvmAccumulatorState::default();
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&state)
            .unwrap()
            .to_vec();
        let deser = rkyv::from_bytes::<EvmAccumulatorState, rkyv::rancor::Error>(&bytes).unwrap();
        assert_eq!(deser, state);
    }

    #[test]
    fn evm_accumulator_with_receipts_round_trip() {
        use crate::precondition::chunking::EvmAccumulatorState;
        use op_alloy_consensus::OpTxType;

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
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&state)
            .unwrap()
            .to_vec();
        let deser = rkyv::from_bytes::<EvmAccumulatorState, rkyv::rancor::Error>(&bytes).unwrap();
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

    #[test]
    fn block_env_round_trip() {
        let env = BlockEnv {
            number: U256::from(42),
            beneficiary: address!("0x1111111111111111111111111111111111111111"),
            timestamp: U256::from(1234),
            gas_limit: 30_000_000,
            basefee: 7,
            difficulty: U256::ZERO,
            prevrandao: Some(B256::repeat_byte(0xAA)),
            blob_excess_gas_and_price: Some(BlobExcessGasAndPrice {
                excess_blob_gas: 1000,
                blob_gasprice: 42,
            }),
        };
        let bytes = to_bytes_with!(BlockEnvRkyv, &env);
        let deser = from_bytes_with!(BlockEnvRkyv, BlockEnv, &bytes);
        assert_eq!(deser.number, env.number);
        assert_eq!(deser.beneficiary, env.beneficiary);
        assert_eq!(deser.timestamp, env.timestamp);
        assert_eq!(deser.gas_limit, env.gas_limit);
        assert_eq!(deser.basefee, env.basefee);
        assert_eq!(deser.difficulty, env.difficulty);
        assert_eq!(deser.prevrandao, env.prevrandao);
        assert_eq!(
            deser.blob_excess_gas_and_price,
            env.blob_excess_gas_and_price
        );
    }

    #[test]
    fn op_block_execution_ctx_round_trip() {
        let ctx = OpBlockExecutionCtx {
            parent_hash: B256::repeat_byte(0xBB),
            parent_beacon_block_root: Some(B256::repeat_byte(0xCC)),
            extra_data: Bytes::from_static(&[1, 2, 3]),
        };
        let bytes = to_bytes_with!(OpBlockExecutionCtxRkyv, &ctx);
        let deser = from_bytes_with!(OpBlockExecutionCtxRkyv, OpBlockExecutionCtx, &bytes);
        assert_eq!(deser.parent_hash, ctx.parent_hash);
        assert_eq!(deser.parent_beacon_block_root, ctx.parent_beacon_block_root);
        assert_eq!(deser.extra_data, ctx.extra_data);
    }
}

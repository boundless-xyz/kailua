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
                            account_id: None,
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

// -- ResultAndStateRkyv, ExecutionResultRkyv, EvmStateRkyv, AccountRkyv --

use alloy_evm::op_revm::OpHaltReason;
use alloy_evm::revm::context_interface::result::{
    ExecutionResult, HaltReason, OutOfGasError, Output, ResultAndState, SuccessReason,
};
use alloy_evm::revm::state::{Account, AccountStatus, EvmState, EvmStorageSlot};
use alloy_primitives::Log;

// -- Enum encoding helpers (SuccessReason, OutOfGasError, HaltReason, OpHaltReason) --

fn success_reason_byte(r: &SuccessReason) -> u8 {
    match r {
        SuccessReason::Stop => 0,
        SuccessReason::Return => 1,
        SuccessReason::SelfDestruct => 2,
    }
}

fn success_reason_from_byte(b: u8) -> SuccessReason {
    match b {
        0 => SuccessReason::Stop,
        1 => SuccessReason::Return,
        2 => SuccessReason::SelfDestruct,
        _ => panic!("invalid SuccessReason byte: {b}"),
    }
}

fn oog_byte(e: &OutOfGasError) -> u8 {
    match e {
        OutOfGasError::Basic => 0,
        OutOfGasError::MemoryLimit => 1,
        OutOfGasError::Memory => 2,
        OutOfGasError::Precompile => 3,
        OutOfGasError::InvalidOperand => 4,
        OutOfGasError::ReentrancySentry => 5,
    }
}

fn oog_from_byte(b: u8) -> OutOfGasError {
    match b {
        0 => OutOfGasError::Basic,
        1 => OutOfGasError::MemoryLimit,
        2 => OutOfGasError::Memory,
        3 => OutOfGasError::Precompile,
        4 => OutOfGasError::InvalidOperand,
        5 => OutOfGasError::ReentrancySentry,
        _ => panic!("invalid OutOfGasError byte: {b}"),
    }
}

type RkyvedHaltReason = (u8, u8, Option<String>);

fn halt_reason_rkyv(r: &HaltReason) -> RkyvedHaltReason {
    match r {
        HaltReason::OutOfGas(e) => (0, oog_byte(e), None),
        HaltReason::OpcodeNotFound => (1, 0, None),
        HaltReason::InvalidFEOpcode => (2, 0, None),
        HaltReason::InvalidJump => (3, 0, None),
        HaltReason::NotActivated => (4, 0, None),
        HaltReason::StackUnderflow => (5, 0, None),
        HaltReason::StackOverflow => (6, 0, None),
        HaltReason::OutOfOffset => (7, 0, None),
        HaltReason::CreateCollision => (8, 0, None),
        HaltReason::PrecompileError => (9, 0, None),
        HaltReason::PrecompileErrorWithContext(s) => (10, 0, Some(s.clone())),
        HaltReason::NonceOverflow => (11, 0, None),
        HaltReason::CreateContractSizeLimit => (12, 0, None),
        HaltReason::CreateContractStartingWithEF => (13, 0, None),
        HaltReason::CreateInitCodeSizeLimit => (14, 0, None),
        HaltReason::OverflowPayment => (15, 0, None),
        HaltReason::StateChangeDuringStaticCall => (16, 0, None),
        HaltReason::CallNotAllowedInsideStatic => (17, 0, None),
        HaltReason::OutOfFunds => (18, 0, None),
        HaltReason::CallTooDeep => (19, 0, None),
    }
}

fn halt_reason_raw(r: RkyvedHaltReason) -> HaltReason {
    match r.0 {
        0 => HaltReason::OutOfGas(oog_from_byte(r.1)),
        1 => HaltReason::OpcodeNotFound,
        2 => HaltReason::InvalidFEOpcode,
        3 => HaltReason::InvalidJump,
        4 => HaltReason::NotActivated,
        5 => HaltReason::StackUnderflow,
        6 => HaltReason::StackOverflow,
        7 => HaltReason::OutOfOffset,
        8 => HaltReason::CreateCollision,
        9 => HaltReason::PrecompileError,
        10 => HaltReason::PrecompileErrorWithContext(r.2.unwrap_or_default()),
        11 => HaltReason::NonceOverflow,
        12 => HaltReason::CreateContractSizeLimit,
        13 => HaltReason::CreateContractStartingWithEF,
        14 => HaltReason::CreateInitCodeSizeLimit,
        15 => HaltReason::OverflowPayment,
        16 => HaltReason::StateChangeDuringStaticCall,
        17 => HaltReason::CallNotAllowedInsideStatic,
        18 => HaltReason::OutOfFunds,
        19 => HaltReason::CallTooDeep,
        _ => panic!("invalid HaltReason discriminant: {}", r.0),
    }
}

type RkyvedOpHaltReason = (u8, Option<RkyvedHaltReason>);

fn op_halt_reason_rkyv(r: &OpHaltReason) -> RkyvedOpHaltReason {
    match r {
        OpHaltReason::Base(h) => (0, Some(halt_reason_rkyv(h))),
        OpHaltReason::FailedDeposit => (1, None),
    }
}

fn op_halt_reason_raw(r: RkyvedOpHaltReason) -> OpHaltReason {
    match r.0 {
        0 => OpHaltReason::Base(halt_reason_raw(r.1.unwrap())),
        1 => OpHaltReason::FailedDeposit,
        _ => panic!("invalid OpHaltReason discriminant: {}", r.0),
    }
}

// -- AccountRkyv --

/// `(original_value_be32, present_value_be32)`.
/// `transaction_id` and `is_cold` are transient and dropped during serialization.
type RkyvedEvmStorageSlot = ([u8; 32], [u8; 32]);

/// Per-`AccountInfo` wire shape (used twice per Account — once for post-tx `info`,
/// once for pre-tx `original_info`).
/// `code` is NOT included (redundant with `code_hash` under `validate_cached_contracts`).
type RkyvedAccountInfo = (u64, [u8; 32], [u8; 32]);

/// `(post_info, pre_info, status_bits, sorted_storage)`.
///
/// Both post-tx `info` and pre-tx `original_info` are serialized — `original_info` is
/// the authoritative "first-load DB value" that `CachedEvm`'s serve-side prestate
/// authentication checks against `db.basic(addr)`. `transaction_id` is transient and
/// defaults to 0 on deserialization.
type RkyvedEvmAccount = (
    RkyvedAccountInfo,
    RkyvedAccountInfo,
    u8,
    Vec<([u8; 32], RkyvedEvmStorageSlot)>,
);

fn account_info_to_rkyv(info: &AccountInfo) -> RkyvedAccountInfo {
    (
        info.nonce,
        info.balance.to_be_bytes::<32>(),
        info.code_hash.0,
    )
}

fn account_info_from_rkyv(r: RkyvedAccountInfo) -> AccountInfo {
    let (nonce, balance, code_hash) = r;
    AccountInfo {
        nonce,
        balance: U256::from_be_bytes(balance),
        code_hash: B256::new(code_hash),
        account_id: None,
        code: None,
    }
}

/// rkyv wrapper for revm's [`Account`] (from `EvmState` traces).
pub struct AccountRkyv;

impl AccountRkyv {
    pub fn rkyv(acct: &Account) -> RkyvedEvmAccount {
        let mut storage: Vec<_> = acct.storage.iter().collect();
        storage.sort_by_key(|(k, _)| *k);
        let storage = storage
            .into_iter()
            .map(|(k, slot)| {
                (
                    k.to_be_bytes::<32>(),
                    (
                        slot.original_value.to_be_bytes::<32>(),
                        slot.present_value.to_be_bytes::<32>(),
                    ),
                )
            })
            .collect();
        (
            account_info_to_rkyv(&acct.info),
            account_info_to_rkyv(acct.original_info.as_ref()),
            acct.status.bits(),
            storage,
        )
    }

    pub fn raw(r: RkyvedEvmAccount) -> Account {
        let (info, original_info, status_bits, storage) = r;
        Account {
            info: account_info_from_rkyv(info),
            original_info: Box::new(account_info_from_rkyv(original_info)),
            transaction_id: 0,
            storage: storage
                .into_iter()
                .map(|(k, (orig, present))| {
                    (
                        U256::from_be_bytes(k),
                        EvmStorageSlot {
                            original_value: U256::from_be_bytes(orig),
                            present_value: U256::from_be_bytes(present),
                            transaction_id: 0,
                            is_cold: false,
                        },
                    )
                })
                .collect(),
            status: AccountStatus::from_bits_retain(status_bits),
        }
    }
}

impl ArchiveWith<Account> for AccountRkyv {
    type Archived = Archived<RkyvedEvmAccount>;
    type Resolver = Resolver<RkyvedEvmAccount>;

    fn resolve_with(field: &Account, resolver: Self::Resolver, out: Place<Self::Archived>) {
        let rkyved = AccountRkyv::rkyv(field);
        <RkyvedEvmAccount as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<Account, S> for AccountRkyv
where
    S: Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    <S as Fallible>::Error: rkyv::rancor::Source,
{
    fn serialize_with(field: &Account, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        let rkyved = AccountRkyv::rkyv(field);
        <RkyvedEvmAccount as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedEvmAccount>, Account, D> for AccountRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: rkyv::rancor::Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedEvmAccount>,
        deserializer: &mut D,
    ) -> Result<Account, D::Error> {
        let rkyved: RkyvedEvmAccount = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(AccountRkyv::raw(rkyved))
    }
}

// -- EvmStateRkyv --

/// `Vec<(address, account)>` sorted by address.
type RkyvedEvmState = Vec<([u8; 20], RkyvedEvmAccount)>;

/// rkyv wrapper for revm's [`EvmState`] (`HashMap<Address, Account>`).
pub struct EvmStateRkyv;

impl EvmStateRkyv {
    pub fn rkyv(state: &EvmState) -> RkyvedEvmState {
        let mut entries: Vec<_> = state.iter().collect();
        entries.sort_by_key(|(addr, _)| *addr);
        entries
            .into_iter()
            .map(|(addr, acct)| (*addr.0, AccountRkyv::rkyv(acct)))
            .collect()
    }

    pub fn raw(entries: RkyvedEvmState) -> EvmState {
        entries
            .into_iter()
            .map(|(addr, acct)| (Address::from(addr), AccountRkyv::raw(acct)))
            .collect()
    }
}

impl ArchiveWith<EvmState> for EvmStateRkyv {
    type Archived = Archived<RkyvedEvmState>;
    type Resolver = Resolver<RkyvedEvmState>;

    fn resolve_with(field: &EvmState, resolver: Self::Resolver, out: Place<Self::Archived>) {
        let rkyved = EvmStateRkyv::rkyv(field);
        <RkyvedEvmState as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<EvmState, S> for EvmStateRkyv
where
    S: Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    <S as Fallible>::Error: rkyv::rancor::Source,
{
    fn serialize_with(field: &EvmState, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        let rkyved = EvmStateRkyv::rkyv(field);
        <RkyvedEvmState as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedEvmState>, EvmState, D> for EvmStateRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: rkyv::rancor::Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedEvmState>,
        deserializer: &mut D,
    ) -> Result<EvmState, D::Error> {
        let rkyved: RkyvedEvmState = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(EvmStateRkyv::raw(rkyved))
    }
}

// -- ExecutionResultRkyv --

type RkyvedLogEntry = ([u8; 20], Vec<[u8; 32]>, Vec<u8>);
type RkyvedOutput = (u8, Vec<u8>, Option<[u8; 20]>);

/// `(disc, success_reason, gas_used, gas_refunded, logs, output, revert_output, halt_reason)`.
/// disc: 0=Success, 1=Revert, 2=Halt.
type RkyvedExecutionResult = (
    u8,
    u8,
    u64,
    u64,
    Vec<RkyvedLogEntry>,
    Option<RkyvedOutput>,
    Option<Vec<u8>>,
    Option<RkyvedOpHaltReason>,
);

/// rkyv wrapper for [`ExecutionResult<OpHaltReason>`].
pub struct ExecutionResultRkyv;

impl ExecutionResultRkyv {
    pub fn rkyv(r: &ExecutionResult<OpHaltReason>) -> RkyvedExecutionResult {
        match r {
            ExecutionResult::Success {
                reason,
                gas_used,
                gas_refunded,
                logs,
                output,
            } => {
                let rkyved_logs = logs
                    .iter()
                    .map(|log| {
                        (
                            *log.address.0,
                            log.data.topics().iter().map(|t| t.0).collect(),
                            log.data.data.to_vec(),
                        )
                    })
                    .collect();
                let rkyved_output = match output {
                    Output::Call(data) => (0, data.to_vec(), None),
                    Output::Create(data, addr) => (1, data.to_vec(), addr.map(|a| *a.0)),
                };
                (
                    0,
                    success_reason_byte(reason),
                    *gas_used,
                    *gas_refunded,
                    rkyved_logs,
                    Some(rkyved_output),
                    None,
                    None,
                )
            }
            ExecutionResult::Revert { gas_used, output } => (
                1,
                0,
                *gas_used,
                0,
                vec![],
                None,
                Some(output.to_vec()),
                None,
            ),
            ExecutionResult::Halt { reason, gas_used } => (
                2,
                0,
                *gas_used,
                0,
                vec![],
                None,
                None,
                Some(op_halt_reason_rkyv(reason)),
            ),
        }
    }

    pub fn raw(r: RkyvedExecutionResult) -> ExecutionResult<OpHaltReason> {
        match r.0 {
            0 => {
                let logs =
                    r.4.into_iter()
                        .map(|(addr, topics, data)| {
                            Log::new_unchecked(
                                Address::from(addr),
                                topics.into_iter().map(B256::new).collect(),
                                Bytes::from(data),
                            )
                        })
                        .collect();
                let (disc, data, addr) = r.5.unwrap();
                let output = match disc {
                    0 => Output::Call(Bytes::from(data)),
                    1 => Output::Create(Bytes::from(data), addr.map(Address::from)),
                    _ => panic!("invalid Output discriminant: {disc}"),
                };
                ExecutionResult::Success {
                    reason: success_reason_from_byte(r.1),
                    gas_used: r.2,
                    gas_refunded: r.3,
                    logs,
                    output,
                }
            }
            1 => ExecutionResult::Revert {
                gas_used: r.2,
                output: Bytes::from(r.6.unwrap()),
            },
            2 => ExecutionResult::Halt {
                reason: op_halt_reason_raw(r.7.unwrap()),
                gas_used: r.2,
            },
            _ => panic!("invalid ExecutionResult discriminant: {}", r.0),
        }
    }
}

impl ArchiveWith<ExecutionResult<OpHaltReason>> for ExecutionResultRkyv {
    type Archived = Archived<RkyvedExecutionResult>;
    type Resolver = Resolver<RkyvedExecutionResult>;

    fn resolve_with(
        field: &ExecutionResult<OpHaltReason>,
        resolver: Self::Resolver,
        out: Place<Self::Archived>,
    ) {
        let rkyved = ExecutionResultRkyv::rkyv(field);
        <RkyvedExecutionResult as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<ExecutionResult<OpHaltReason>, S> for ExecutionResultRkyv
where
    S: Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    <S as Fallible>::Error: rkyv::rancor::Source,
{
    fn serialize_with(
        field: &ExecutionResult<OpHaltReason>,
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        let rkyved = ExecutionResultRkyv::rkyv(field);
        <RkyvedExecutionResult as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedExecutionResult>, ExecutionResult<OpHaltReason>, D>
    for ExecutionResultRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: rkyv::rancor::Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedExecutionResult>,
        deserializer: &mut D,
    ) -> Result<ExecutionResult<OpHaltReason>, D::Error> {
        let rkyved: RkyvedExecutionResult = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(ExecutionResultRkyv::raw(rkyved))
    }
}

// -- ResultAndStateRkyv --

/// `(execution_result, evm_state_entries)`.
type RkyvedResultAndState = (RkyvedExecutionResult, RkyvedEvmState);

/// rkyv wrapper for [`ResultAndState<OpHaltReason>`].
///
/// Composes [`ExecutionResultRkyv`] and [`EvmStateRkyv`].
/// Used by `Chunk.results` via `rkyv::with::Map<ResultAndStateRkyv>`.
pub struct ResultAndStateRkyv;

impl ResultAndStateRkyv {
    pub fn rkyv(ras: &ResultAndState<OpHaltReason>) -> RkyvedResultAndState {
        (
            ExecutionResultRkyv::rkyv(&ras.result),
            EvmStateRkyv::rkyv(&ras.state),
        )
    }

    pub fn raw(r: RkyvedResultAndState) -> ResultAndState<OpHaltReason> {
        ResultAndState {
            result: ExecutionResultRkyv::raw(r.0),
            state: EvmStateRkyv::raw(r.1),
        }
    }
}

impl ArchiveWith<ResultAndState<OpHaltReason>> for ResultAndStateRkyv {
    type Archived = Archived<RkyvedResultAndState>;
    type Resolver = Resolver<RkyvedResultAndState>;

    fn resolve_with(
        field: &ResultAndState<OpHaltReason>,
        resolver: Self::Resolver,
        out: Place<Self::Archived>,
    ) {
        let rkyved = ResultAndStateRkyv::rkyv(field);
        <RkyvedResultAndState as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<ResultAndState<OpHaltReason>, S> for ResultAndStateRkyv
where
    S: Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    <S as Fallible>::Error: rkyv::rancor::Source,
{
    fn serialize_with(
        field: &ResultAndState<OpHaltReason>,
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        let rkyved = ResultAndStateRkyv::rkyv(field);
        <RkyvedResultAndState as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedResultAndState>, ResultAndState<OpHaltReason>, D>
    for ResultAndStateRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: rkyv::rancor::Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedResultAndState>,
        deserializer: &mut D,
    ) -> Result<ResultAndState<OpHaltReason>, D::Error> {
        let rkyved: RkyvedResultAndState = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(ResultAndStateRkyv::raw(rkyved))
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
            account_id: None,
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
                    account_id: None,
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

    // -- ResultAndStateRkyv tests --

    fn make_success_result() -> ResultAndState<OpHaltReason> {
        let addr = address!("0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        let code = Bytecode::new_raw(Bytes::from_static(&[0x60, 0x00]));
        let mut storage = alloy_evm::revm::primitives::HashMap::default();
        storage.insert(
            U256::from(1),
            EvmStorageSlot::new_changed(U256::from(0), U256::from(42), 0),
        );
        let mut state = alloy_evm::revm::state::EvmState::default();
        state.insert(
            addr,
            Account {
                info: AccountInfo {
                    nonce: 5,
                    balance: U256::from(1000),
                    code_hash: code.hash_slow(),
                    account_id: None,
                    code: Some(code),
                },
                original_info: Box::new(AccountInfo::default()),
                transaction_id: 0,
                storage,
                status: AccountStatus::Touched | AccountStatus::Created,
            },
        );
        ResultAndState {
            result: ExecutionResult::Success {
                reason: SuccessReason::Return,
                gas_used: 21000,
                gas_refunded: 100,
                logs: vec![Log::new_unchecked(
                    addr,
                    vec![B256::repeat_byte(0xAA)],
                    Bytes::from_static(&[0x01, 0x02]),
                )],
                output: Output::Call(Bytes::from_static(&[0xDE, 0xAD])),
            },
            state,
        }
    }

    fn make_revert_result() -> ResultAndState<OpHaltReason> {
        ResultAndState {
            result: ExecutionResult::Revert {
                gas_used: 50000,
                output: Bytes::from_static(&[0x08, 0xc3, 0x79, 0xa0]),
            },
            state: Default::default(),
        }
    }

    fn make_halt_result() -> ResultAndState<OpHaltReason> {
        ResultAndState {
            result: ExecutionResult::Halt {
                reason: OpHaltReason::Base(HaltReason::OutOfGas(OutOfGasError::Basic)),
                gas_used: 100000,
            },
            state: Default::default(),
        }
    }

    fn make_halt_failed_deposit() -> ResultAndState<OpHaltReason> {
        let addr = address!("0xBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB");
        let mut state = alloy_evm::revm::state::EvmState::default();
        state.insert(
            addr,
            Account {
                info: AccountInfo {
                    nonce: 1,
                    balance: U256::from(500),
                    code_hash: B256::ZERO,
                    account_id: None,
                    code: None,
                },
                original_info: Box::new(AccountInfo::default()),
                transaction_id: 0,
                storage: Default::default(),
                status: AccountStatus::Touched,
            },
        );
        ResultAndState {
            result: ExecutionResult::Halt {
                reason: OpHaltReason::FailedDeposit,
                gas_used: 0,
            },
            state,
        }
    }

    #[test]
    fn result_and_state_success_round_trip() {
        let ras = make_success_result();
        let bytes = to_bytes_with!(ResultAndStateRkyv, &ras);
        let deser = from_bytes_with!(ResultAndStateRkyv, ResultAndState<OpHaltReason>, &bytes);

        match &deser.result {
            ExecutionResult::Success {
                reason,
                gas_used,
                gas_refunded,
                logs,
                output,
            } => {
                assert_eq!(*reason, SuccessReason::Return);
                assert_eq!(*gas_used, 21000);
                assert_eq!(*gas_refunded, 100);
                assert_eq!(logs.len(), 1);
                assert_eq!(
                    logs[0].address,
                    address!("0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
                );
                assert!(matches!(output, Output::Call(d) if d.as_ref() == [0xDE, 0xAD]));
            }
            _ => panic!("expected Success"),
        }
        assert_eq!(deser.state.len(), 1);
        let acct = deser
            .state
            .get(&address!("0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"))
            .unwrap();
        assert_eq!(acct.info.nonce, 5);
        assert_eq!(acct.info.balance, U256::from(1000));
        // code is not serialized (redundant with code_hash under
        // validate_cached_contracts) — only code_hash round-trips.
        assert!(acct.info.code.is_none());
        assert!(acct.status.contains(AccountStatus::Touched));
        assert!(acct.status.contains(AccountStatus::Created));
        let slot = acct.storage.get(&U256::from(1)).unwrap();
        assert_eq!(slot.original_value, U256::from(0));
        assert_eq!(slot.present_value, U256::from(42));
    }

    #[test]
    fn result_and_state_revert_round_trip() {
        let ras = make_revert_result();
        let bytes = to_bytes_with!(ResultAndStateRkyv, &ras);
        let deser = from_bytes_with!(ResultAndStateRkyv, ResultAndState<OpHaltReason>, &bytes);

        match &deser.result {
            ExecutionResult::Revert { gas_used, output } => {
                assert_eq!(*gas_used, 50000);
                assert_eq!(output.as_ref(), &[0x08, 0xc3, 0x79, 0xa0]);
            }
            _ => panic!("expected Revert"),
        }
        assert!(deser.state.is_empty());
    }

    #[test]
    fn result_and_state_halt_round_trip() {
        let ras = make_halt_result();
        let bytes = to_bytes_with!(ResultAndStateRkyv, &ras);
        let deser = from_bytes_with!(ResultAndStateRkyv, ResultAndState<OpHaltReason>, &bytes);

        match &deser.result {
            ExecutionResult::Halt { reason, gas_used } => {
                assert_eq!(
                    *reason,
                    OpHaltReason::Base(HaltReason::OutOfGas(OutOfGasError::Basic))
                );
                assert_eq!(*gas_used, 100000);
            }
            _ => panic!("expected Halt"),
        }
    }

    #[test]
    fn result_and_state_failed_deposit_round_trip() {
        let ras = make_halt_failed_deposit();
        let bytes = to_bytes_with!(ResultAndStateRkyv, &ras);
        let deser = from_bytes_with!(ResultAndStateRkyv, ResultAndState<OpHaltReason>, &bytes);

        match &deser.result {
            ExecutionResult::Halt { reason, gas_used } => {
                assert_eq!(*reason, OpHaltReason::FailedDeposit);
                assert_eq!(*gas_used, 0);
            }
            _ => panic!("expected Halt/FailedDeposit"),
        }
        assert_eq!(deser.state.len(), 1);
    }

    #[test]
    fn result_and_state_create_output_round_trip() {
        let created_addr = address!("0xCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC");
        let ras = ResultAndState {
            result: ExecutionResult::Success {
                reason: SuccessReason::Return,
                gas_used: 53000,
                gas_refunded: 0,
                logs: vec![],
                output: Output::Create(Bytes::from_static(&[0x60, 0x00]), Some(created_addr)),
            },
            state: Default::default(),
        };
        let bytes = to_bytes_with!(ResultAndStateRkyv, &ras);
        let deser = from_bytes_with!(ResultAndStateRkyv, ResultAndState<OpHaltReason>, &bytes);

        match &deser.result {
            ExecutionResult::Success { output, .. } => {
                assert!(matches!(
                    output,
                    Output::Create(data, Some(addr))
                        if data.as_ref() == [0x60, 0x00] && *addr == created_addr
                ));
            }
            _ => panic!("expected Success with Create output"),
        }
    }

    #[test]
    fn result_and_state_create_output_none_address_round_trip() {
        let ras = ResultAndState {
            result: ExecutionResult::Success {
                reason: SuccessReason::Return,
                gas_used: 53000,
                gas_refunded: 0,
                logs: vec![],
                output: Output::Create(Bytes::from_static(&[0x60, 0x00]), None),
            },
            state: Default::default(),
        };
        let bytes = to_bytes_with!(ResultAndStateRkyv, &ras);
        let deser = from_bytes_with!(ResultAndStateRkyv, ResultAndState<OpHaltReason>, &bytes);

        match &deser.result {
            ExecutionResult::Success { output, .. } => {
                assert!(matches!(
                    output,
                    Output::Create(data, None) if data.as_ref() == [0x60, 0x00]
                ));
            }
            _ => panic!("expected Success with Create output"),
        }
    }

    #[test]
    fn result_and_state_all_halt_reasons_round_trip() {
        let reasons = vec![
            OpHaltReason::Base(HaltReason::OutOfGas(OutOfGasError::Basic)),
            OpHaltReason::Base(HaltReason::OutOfGas(OutOfGasError::MemoryLimit)),
            OpHaltReason::Base(HaltReason::OutOfGas(OutOfGasError::Memory)),
            OpHaltReason::Base(HaltReason::OutOfGas(OutOfGasError::Precompile)),
            OpHaltReason::Base(HaltReason::OutOfGas(OutOfGasError::InvalidOperand)),
            OpHaltReason::Base(HaltReason::OutOfGas(OutOfGasError::ReentrancySentry)),
            OpHaltReason::Base(HaltReason::OpcodeNotFound),
            OpHaltReason::Base(HaltReason::InvalidFEOpcode),
            OpHaltReason::Base(HaltReason::InvalidJump),
            OpHaltReason::Base(HaltReason::NotActivated),
            OpHaltReason::Base(HaltReason::StackUnderflow),
            OpHaltReason::Base(HaltReason::StackOverflow),
            OpHaltReason::Base(HaltReason::OutOfOffset),
            OpHaltReason::Base(HaltReason::CreateCollision),
            OpHaltReason::Base(HaltReason::PrecompileError),
            OpHaltReason::Base(HaltReason::PrecompileErrorWithContext("test error".into())),
            OpHaltReason::Base(HaltReason::NonceOverflow),
            OpHaltReason::Base(HaltReason::CreateContractSizeLimit),
            OpHaltReason::Base(HaltReason::CreateContractStartingWithEF),
            OpHaltReason::Base(HaltReason::CreateInitCodeSizeLimit),
            OpHaltReason::Base(HaltReason::OverflowPayment),
            OpHaltReason::Base(HaltReason::StateChangeDuringStaticCall),
            OpHaltReason::Base(HaltReason::CallNotAllowedInsideStatic),
            OpHaltReason::Base(HaltReason::OutOfFunds),
            OpHaltReason::Base(HaltReason::CallTooDeep),
            OpHaltReason::FailedDeposit,
        ];
        for reason in reasons {
            let ras = ResultAndState {
                result: ExecutionResult::Halt {
                    reason: reason.clone(),
                    gas_used: 99,
                },
                state: Default::default(),
            };
            let bytes = to_bytes_with!(ResultAndStateRkyv, &ras);
            let deser = from_bytes_with!(ResultAndStateRkyv, ResultAndState<OpHaltReason>, &bytes);
            match deser.result {
                ExecutionResult::Halt {
                    reason: deser_reason,
                    ..
                } => assert_eq!(deser_reason, reason),
                _ => panic!("expected Halt"),
            }
        }
    }

    #[test]
    fn result_and_state_account_lifecycle_states_round_trip() {
        let addr = address!("0xDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD");
        for status in [
            AccountStatus::empty(),
            AccountStatus::Touched,
            AccountStatus::Created,
            AccountStatus::CreatedLocal,
            AccountStatus::SelfDestructed,
            AccountStatus::SelfDestructedLocal,
            AccountStatus::LoadedAsNotExisting,
            AccountStatus::Cold,
            AccountStatus::Touched | AccountStatus::Created,
            AccountStatus::SelfDestructed | AccountStatus::Touched,
            AccountStatus::CreatedLocal | AccountStatus::Created | AccountStatus::Touched,
        ] {
            let mut state = alloy_evm::revm::state::EvmState::default();
            state.insert(
                addr,
                Account {
                    info: AccountInfo::default(),
                    original_info: Box::new(AccountInfo::default()),
                    transaction_id: 0,
                    storage: Default::default(),
                    status,
                },
            );
            let ras = ResultAndState {
                result: ExecutionResult::Success {
                    reason: SuccessReason::Stop,
                    gas_used: 0,
                    gas_refunded: 0,
                    logs: vec![],
                    output: Output::Call(Bytes::new()),
                },
                state,
            };
            let bytes = to_bytes_with!(ResultAndStateRkyv, &ras);
            let deser = from_bytes_with!(ResultAndStateRkyv, ResultAndState<OpHaltReason>, &bytes);
            let acct = deser.state.get(&addr).unwrap();
            assert_eq!(acct.status, status, "status mismatch for {status:?}");
        }
    }
}

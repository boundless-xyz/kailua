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

use alloy_evm::revm::context::BlockEnv;
use alloy_evm::revm::context_interface::block::BlobExcessGasAndPrice;
use alloy_evm::revm::database::states::{CacheAccount, PlainAccount};
use alloy_evm::revm::database::{AccountStatus as CacheAccountStatus, CacheState};
use alloy_evm::revm::state::{AccountInfo, Bytecode};
use alloy_op_evm::block::OpBlockExecutionCtx;
use alloy_primitives::{Address, Bytes, B256, U256};
use rkyv::rancor::Fallible;
use rkyv::with::{ArchiveWith, DeserializeWith, SerializeWith};
use rkyv::{Archive, Archived, Place, Resolver};

// -- CacheStateRkyv --

/// `(info, sorted_storage)`. `info` reuses [`RkyvedAccountInfo`]; bytecode is
/// carried via the separate `contracts` map (bound to content by `code_hash`).
type RkyvedPlainAccount = (RkyvedAccountInfo, Vec<([u8; 32], [u8; 32])>);

/// `(address, Option<plain>, cache_account_status_byte)`. `None` plain marks a
/// `CacheAccount` whose `account` field is `None` (e.g. `LoadedNotExisting`).
type RkyvedCacheAccount = ([u8; 20], Option<RkyvedPlainAccount>, u8);

/// `(sorted_accounts, sorted_contracts, has_state_clear)`.
pub type RkyvedCacheState = (Vec<RkyvedCacheAccount>, Vec<([u8; 32], Vec<u8>)>, bool);

/// Encodes [`CacheAccountStatus`] as a single canonical byte for rkyv.
fn cache_account_status_byte(s: &CacheAccountStatus) -> u8 {
    match s {
        CacheAccountStatus::LoadedNotExisting => 0,
        CacheAccountStatus::Loaded => 1,
        CacheAccountStatus::LoadedEmptyEIP161 => 2,
        CacheAccountStatus::InMemoryChange => 3,
        CacheAccountStatus::Changed => 4,
        CacheAccountStatus::Destroyed => 5,
        CacheAccountStatus::DestroyedChanged => 6,
        CacheAccountStatus::DestroyedAgain => 7,
    }
}

fn cache_account_status_from_byte(b: u8) -> CacheAccountStatus {
    match b {
        0 => CacheAccountStatus::LoadedNotExisting,
        1 => CacheAccountStatus::Loaded,
        2 => CacheAccountStatus::LoadedEmptyEIP161,
        3 => CacheAccountStatus::InMemoryChange,
        4 => CacheAccountStatus::Changed,
        5 => CacheAccountStatus::Destroyed,
        6 => CacheAccountStatus::DestroyedChanged,
        7 => CacheAccountStatus::DestroyedAgain,
        _ => panic!("invalid CacheAccountStatus byte: {b}"),
    }
}

/// rkyv wrapper for revm's [`CacheState`].
pub struct CacheStateRkyv;

impl CacheStateRkyv {
    pub fn rkyv(cs: &CacheState) -> RkyvedCacheState {
        // Accounts (sorted by address)
        let mut accounts: Vec<_> = cs.accounts.iter().collect();
        accounts.sort_by_key(|(addr, _)| *addr);
        let accounts = accounts
            .into_iter()
            .map(|(addr, ca)| {
                let plain = ca.account.as_ref().map(|p| {
                    let mut storage: Vec<_> = p.storage.iter().collect();
                    storage.sort_by_key(|(k, _)| *k);
                    let storage = storage
                        .into_iter()
                        .map(|(k, v)| (k.to_be_bytes::<32>(), v.to_be_bytes::<32>()))
                        .collect();
                    (account_info_to_rkyv(&p.info), storage)
                });
                (*addr.0, plain, cache_account_status_byte(&ca.status))
            })
            .collect();

        // Contracts (sorted by code_hash)
        let mut contracts: Vec<_> = cs.contracts.iter().collect();
        contracts.sort_by_key(|(hash, _)| *hash);
        let contracts = contracts
            .into_iter()
            .map(|(hash, bytecode)| (hash.0, bytecode.original_bytes().to_vec()))
            .collect();

        (accounts, contracts, cs.has_state_clear)
    }

    pub fn raw(rkyved: RkyvedCacheState) -> CacheState {
        let (accounts, contracts, has_state_clear) = rkyved;

        let accounts = accounts
            .into_iter()
            .map(|(addr, plain, status_byte)| {
                let account = plain.map(|(info, storage)| PlainAccount {
                    info: account_info_from_rkyv(info),
                    storage: storage
                        .into_iter()
                        .map(|(k, v)| (U256::from_be_bytes(k), U256::from_be_bytes(v)))
                        .collect(),
                });
                (
                    Address::from(addr),
                    CacheAccount {
                        account,
                        status: cache_account_status_from_byte(status_byte),
                    },
                )
            })
            .collect();

        let contracts = contracts
            .into_iter()
            .map(|(hash, raw)| (B256::new(hash), Bytecode::new_raw(Bytes::from(raw))))
            .collect();

        CacheState {
            accounts,
            contracts,
            has_state_clear,
        }
    }
}

impl ArchiveWith<CacheState> for CacheStateRkyv {
    type Archived = Archived<RkyvedCacheState>;
    type Resolver = Resolver<RkyvedCacheState>;

    fn resolve_with(field: &CacheState, resolver: Self::Resolver, out: Place<Self::Archived>) {
        let rkyved = CacheStateRkyv::rkyv(field);
        <RkyvedCacheState as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<CacheState, S> for CacheStateRkyv
where
    S: Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    <S as Fallible>::Error: rkyv::rancor::Source,
{
    fn serialize_with(field: &CacheState, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        let rkyved = CacheStateRkyv::rkyv(field);
        <RkyvedCacheState as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedCacheState>, CacheState, D> for CacheStateRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: rkyv::rancor::Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedCacheState>,
        deserializer: &mut D,
    ) -> Result<CacheState, D::Error> {
        let rkyved: RkyvedCacheState = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(CacheStateRkyv::raw(rkyved))
    }
}

// -- BlockEnvRkyv --

/// rkyv-friendly mirror of [`BlobExcessGasAndPrice`]. The upstream type lacks
/// rkyv support, so we project it through this named struct to keep the
/// archived wire shape self-documenting (vs. an opaque `(u64, u128)` tuple).
/// Field order/types match the tuple it replaces, so the archived layout is
/// unchanged — existing serialized data stays readable.
#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct RkyvedBlobGasAndPrice {
    pub excess_blob_gas: u64,
    pub blob_gasprice: [u8; 16],
}

/// (number, beneficiary, timestamp, gas_limit, basefee, difficulty, prevrandao, blob_excess_gas_and_price)
type RkyvedBlockEnv = (
    U256,
    Address,
    U256,
    u64,
    u64,
    U256,
    Option<B256>,
    Option<RkyvedBlobGasAndPrice>,
);

/// rkyv wrapper for revm's [`BlockEnv`].
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
                .map(|b| RkyvedBlobGasAndPrice {
                    excess_blob_gas: b.excess_blob_gas,
                    blob_gasprice: b.blob_gasprice.to_be_bytes(),
                }),
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
            blob_excess_gas_and_price: r.7.map(|b| BlobExcessGasAndPrice {
                excess_blob_gas: b.excess_blob_gas,
                blob_gasprice: u128::from_be_bytes(b.blob_gasprice),
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
    ExecutionResult, HaltReason, OutOfGasError, Output, SuccessReason,
};
use alloy_evm::revm::state::{AccountStatus, EvmStorageSlot};
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

// -- AccountInfoRkyv / AccountStatusRkyv / EvmStorageSlotRkyv --

/// `with`-wrapper for revm's [`AccountInfo`]. Archives via the existing
/// `RkyvedAccountInfo = (u64, [u8; 32], [u8; 32])` tuple shape used by
/// `AccountRkyv`. Drops `account_id` and `code` (consistent with
/// `AccountRkyv` — `code_hash` binds bytecode content via
/// `validate_cached_contracts`).
pub struct AccountInfoRkyv;

impl ArchiveWith<AccountInfo> for AccountInfoRkyv {
    type Archived = Archived<RkyvedAccountInfo>;
    type Resolver = Resolver<RkyvedAccountInfo>;

    fn resolve_with(field: &AccountInfo, resolver: Self::Resolver, out: Place<Self::Archived>) {
        let rkyved = account_info_to_rkyv(field);
        <RkyvedAccountInfo as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<AccountInfo, S> for AccountInfoRkyv
where
    S: Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    <S as Fallible>::Error: rkyv::rancor::Source,
{
    fn serialize_with(field: &AccountInfo, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        let rkyved = account_info_to_rkyv(field);
        <RkyvedAccountInfo as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedAccountInfo>, AccountInfo, D> for AccountInfoRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: rkyv::rancor::Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedAccountInfo>,
        deserializer: &mut D,
    ) -> Result<AccountInfo, D::Error> {
        let rkyved: RkyvedAccountInfo = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(account_info_from_rkyv(rkyved))
    }
}

/// `with`-wrapper for revm's [`AccountStatus`]. Archives as
/// the bitflag's `u8` representation — matches what `AccountRkyv` already
/// emits for the `status` field.
pub struct AccountStatusRkyv;

impl ArchiveWith<AccountStatus> for AccountStatusRkyv {
    type Archived = Archived<u8>;
    type Resolver = Resolver<u8>;

    fn resolve_with(field: &AccountStatus, resolver: Self::Resolver, out: Place<Self::Archived>) {
        <u8 as Archive>::resolve(&field.bits(), resolver, out);
    }
}

impl<S> SerializeWith<AccountStatus, S> for AccountStatusRkyv
where
    S: Fallible + ?Sized,
    <S as Fallible>::Error: rkyv::rancor::Source,
{
    fn serialize_with(
        field: &AccountStatus,
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        <u8 as rkyv::Serialize<S>>::serialize(&field.bits(), serializer)
    }
}

impl<D> DeserializeWith<Archived<u8>, AccountStatus, D> for AccountStatusRkyv
where
    D: Fallible + ?Sized,
{
    fn deserialize_with(field: &Archived<u8>, _: &mut D) -> Result<AccountStatus, D::Error> {
        Ok(AccountStatus::from_bits_retain(*field))
    }
}

/// `with`-wrapper for revm's [`EvmStorageSlot`].
/// Archives as `(original_be32, present_be32)` — the shape `AccountRkyv`
/// already emits per-slot. Transient fields (`transaction_id`, `is_cold`)
/// are dropped on archive and reset to `(0, false)` on deserialize, matching
/// `AccountRkyv`'s existing behavior.
pub struct EvmStorageSlotRkyv;

impl ArchiveWith<EvmStorageSlot> for EvmStorageSlotRkyv {
    type Archived = Archived<RkyvedEvmStorageSlot>;
    type Resolver = Resolver<RkyvedEvmStorageSlot>;

    fn resolve_with(field: &EvmStorageSlot, resolver: Self::Resolver, out: Place<Self::Archived>) {
        let rkyved = (
            field.original_value.to_be_bytes::<32>(),
            field.present_value.to_be_bytes::<32>(),
        );
        <RkyvedEvmStorageSlot as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<EvmStorageSlot, S> for EvmStorageSlotRkyv
where
    S: Fallible + ?Sized,
    <S as Fallible>::Error: rkyv::rancor::Source,
{
    fn serialize_with(
        field: &EvmStorageSlot,
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        let rkyved = (
            field.original_value.to_be_bytes::<32>(),
            field.present_value.to_be_bytes::<32>(),
        );
        <RkyvedEvmStorageSlot as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedEvmStorageSlot>, EvmStorageSlot, D> for EvmStorageSlotRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: rkyv::rancor::Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedEvmStorageSlot>,
        deserializer: &mut D,
    ) -> Result<EvmStorageSlot, D::Error> {
        let (orig, present): RkyvedEvmStorageSlot =
            rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(EvmStorageSlot {
            original_value: U256::from_be_bytes(orig),
            present_value: U256::from_be_bytes(present),
            transaction_id: 0,
            is_cold: false,
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{from_bytes_with, to_bytes_with};
    use alloy_primitives::address;

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

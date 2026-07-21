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

//! rkyv `ArchiveWith` wrappers for revm's `Cache` and `EvmAccumulatorState`.

use alloy_evm::revm::context::BlockEnv;
use alloy_evm::revm::context_interface::block::BlobExcessGasAndPrice;
use alloy_evm::revm::database::states::{CacheAccount, PlainAccount};
use alloy_evm::revm::database::{AccountStatus as CacheAccountStatus, CacheState};
use alloy_evm::revm::state::{AccountInfo, Bytecode};
use alloy_op_evm::block::{OpBlockExecutionCtx, PostExecMode};
use alloy_primitives::{Address, B256, Bytes, U256};
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

/// `(sorted_accounts, sorted_contracts)`.
pub type RkyvedCacheState = (Vec<RkyvedCacheAccount>, Vec<([u8; 32], Vec<u8>)>);

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
    /// Converts the value into its tuple encoding, sorting accounts, storage, and contracts by
    /// key for determinism.
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

        (accounts, contracts)
    }

    /// Reconstructs the value from its tuple encoding.
    pub fn raw(rkyved: RkyvedCacheState) -> CacheState {
        let (accounts, contracts) = rkyved;

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
    /// The block's excess blob gas.
    pub excess_blob_gas: u64,
    /// Big-endian encoding of the block's blob gas price.
    pub blob_gasprice: [u8; 16],
}

/// (number, beneficiary, timestamp, gas_limit, basefee, difficulty, prevrandao, blob_excess_gas_and_price, slot_num)
type RkyvedBlockEnv = (
    U256,
    Address,
    U256,
    u64,
    u64,
    U256,
    Option<B256>,
    Option<RkyvedBlobGasAndPrice>,
    u64,
);

/// rkyv wrapper for revm's [`BlockEnv`].
pub struct BlockEnvRkyv;

impl BlockEnvRkyv {
    /// Converts the value into its tuple encoding.
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
            env.slot_num,
        )
    }

    /// Reconstructs the value from its tuple encoding.
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
            slot_num: r.8,
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

/// (parent_hash, parent_beacon_block_root, extra_data, post_exec_mode_disc)
/// post_exec_mode_disc: 0=Disabled, 1=Produce. `Verify` is unsupported in the witness.
type RkyvedOpBlockExecutionCtx = (B256, Option<B256>, Bytes, u8);

/// Encodes [`PostExecMode`] as a single byte. `Verify` carries an embedded
/// post-exec payload that never occurs on the proving path, so it is rejected.
///
/// SDM future work: once kona schedules SDM (`RollupConfig::is_sdm_active` is
/// hardcoded `false` in v1.5.2), every canonical block ctx becomes
/// `Verify(payload)` and this panic makes partial-execution witnesses
/// unserializable. Preferred fix: keep the payload out of the witness and
/// reconstruct it in-guest from the block's transactions (kona parses it from
/// them via `parse_post_exec_payload_from_transactions`), so it never enters
/// the trust surface. See the SDM canonical note on `CachedEvm`'s
/// `PostExecEvm` impl in `evm/cached.rs`.
pub(crate) fn post_exec_mode_byte(m: &PostExecMode) -> u8 {
    match m {
        PostExecMode::Disabled => 0,
        PostExecMode::Produce => 1,
        PostExecMode::Verify(_) => {
            panic!("PostExecMode::Verify is unsupported in the execution witness")
        }
    }
}

fn post_exec_mode_from_byte(b: u8) -> PostExecMode {
    match b {
        0 => PostExecMode::Disabled,
        1 => PostExecMode::Produce,
        _ => panic!("invalid PostExecMode byte: {b}"),
    }
}

/// rkyv wrapper for [`OpBlockExecutionCtx`].
///
/// Archives as a tuple of rkyv-native types via `RkyvedOpBlockExecutionCtx`.
/// All fields have native rkyv support via the `alloy-primitives` `rkyv` feature.
pub struct OpBlockExecutionCtxRkyv;

impl OpBlockExecutionCtxRkyv {
    /// Converts the value into its tuple encoding.
    pub fn rkyv(ctx: &OpBlockExecutionCtx) -> RkyvedOpBlockExecutionCtx {
        (
            ctx.parent_hash,
            ctx.parent_beacon_block_root,
            ctx.extra_data.clone(),
            post_exec_mode_byte(&ctx.post_exec_mode),
        )
    }

    /// Reconstructs the value from its tuple encoding.
    pub fn raw(r: RkyvedOpBlockExecutionCtx) -> OpBlockExecutionCtx {
        OpBlockExecutionCtx {
            parent_hash: r.0,
            parent_beacon_block_root: r.1,
            extra_data: r.2,
            post_exec_mode: post_exec_mode_from_byte(r.3),
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

use alloy_evm::revm::context_interface::result::{
    ExecutionResult, HaltReason, OutOfGasError, Output, ResultGas, SuccessReason,
};
use alloy_evm::revm::state::{AccountStatus, EvmStorageSlot};
use alloy_primitives::Log;
use op_revm::OpHaltReason;

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

/// revm's [`ResultGas`] as `(total_gas_spent, state_gas_spent, refunded, floor_gas)`.
type RkyvedResultGas = (u64, u64, u64, u64);

fn result_gas_rkyv(g: &ResultGas) -> RkyvedResultGas {
    (
        g.total_gas_spent(),
        g.state_gas_spent(),
        g.inner_refunded(),
        g.floor_gas(),
    )
}

fn result_gas_raw(r: RkyvedResultGas) -> ResultGas {
    ResultGas::default()
        .with_total_gas_spent(r.0)
        .with_state_gas_spent(r.1)
        .with_refunded(r.2)
        .with_floor_gas(r.3)
}

fn logs_rkyv(logs: &[Log]) -> Vec<RkyvedLogEntry> {
    logs.iter()
        .map(|log| {
            (
                *log.address.0,
                log.data.topics().iter().map(|t| t.0).collect(),
                log.data.data.to_vec(),
            )
        })
        .collect()
}

fn logs_raw(logs: Vec<RkyvedLogEntry>) -> Vec<Log> {
    logs.into_iter()
        .map(|(addr, topics, data)| {
            Log::new_unchecked(
                Address::from(addr),
                topics.into_iter().map(B256::new).collect(),
                Bytes::from(data),
            )
        })
        .collect()
}

/// `(disc, success_reason, gas, logs, output, revert_output, halt_reason)`.
/// disc: 0=Success, 1=Revert, 2=Halt. `logs` is present for every variant.
type RkyvedExecutionResult = (
    u8,
    u8,
    RkyvedResultGas,
    Vec<RkyvedLogEntry>,
    Option<RkyvedOutput>,
    Option<Vec<u8>>,
    Option<RkyvedOpHaltReason>,
);

/// rkyv wrapper for [`ExecutionResult<OpHaltReason>`].
pub struct ExecutionResultRkyv;

impl ExecutionResultRkyv {
    /// Converts the value into its tuple encoding.
    pub fn rkyv(r: &ExecutionResult<OpHaltReason>) -> RkyvedExecutionResult {
        match r {
            ExecutionResult::Success {
                reason,
                gas,
                logs,
                output,
            } => {
                let rkyved_output = match output {
                    Output::Call(data) => (0, data.to_vec(), None),
                    Output::Create(data, addr) => (1, data.to_vec(), addr.map(|a| *a.0)),
                };
                (
                    0,
                    success_reason_byte(reason),
                    result_gas_rkyv(gas),
                    logs_rkyv(logs),
                    Some(rkyved_output),
                    None,
                    None,
                )
            }
            ExecutionResult::Revert { gas, logs, output } => (
                1,
                0,
                result_gas_rkyv(gas),
                logs_rkyv(logs),
                None,
                Some(output.to_vec()),
                None,
            ),
            ExecutionResult::Halt { reason, gas, logs } => (
                2,
                0,
                result_gas_rkyv(gas),
                logs_rkyv(logs),
                None,
                None,
                Some(op_halt_reason_rkyv(reason)),
            ),
        }
    }

    /// Reconstructs the value from its tuple encoding, panicking on invalid discriminants.
    pub fn raw(r: RkyvedExecutionResult) -> ExecutionResult<OpHaltReason> {
        match r.0 {
            0 => {
                let (disc, data, addr) = r.4.unwrap();
                let output = match disc {
                    0 => Output::Call(Bytes::from(data)),
                    1 => Output::Create(Bytes::from(data), addr.map(Address::from)),
                    _ => panic!("invalid Output discriminant: {disc}"),
                };
                ExecutionResult::Success {
                    reason: success_reason_from_byte(r.1),
                    gas: result_gas_raw(r.2),
                    logs: logs_raw(r.3),
                    output,
                }
            }
            1 => ExecutionResult::Revert {
                gas: result_gas_raw(r.2),
                logs: logs_raw(r.3),
                output: Bytes::from(r.5.unwrap()),
            },
            2 => ExecutionResult::Halt {
                reason: op_halt_reason_raw(r.6.unwrap()),
                gas: result_gas_raw(r.2),
                logs: logs_raw(r.3),
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
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::{from_bytes_with, to_bytes_with};
    use alloy_primitives::address;

    fn info_with(nonce: u64, balance: U256, code_hash: B256) -> AccountInfo {
        AccountInfo {
            nonce,
            balance,
            code_hash,
            account_id: Some(7),
            code: Some(Bytecode::new_raw(Bytes::from_static(&[0x60, 0x00]))),
        }
    }

    fn slot(orig: u64, present: u64) -> EvmStorageSlot {
        EvmStorageSlot {
            original_value: U256::from(orig),
            present_value: U256::from(present),
            transaction_id: 99,
            is_cold: true,
        }
    }

    /// Round-trip-aware equality: persistent fields are compared field-wise,
    /// and the wrapper is also asserted to drop the transient `account_id` and
    /// `code` regardless of input — the latter is the central invariant of
    /// `AccountInfoRkyv` and applies anywhere it round-trips.
    fn assert_account_info_round_trip(deser: &AccountInfo, original: &AccountInfo) {
        assert_eq!(deser.nonce, original.nonce);
        assert_eq!(deser.balance, original.balance);
        assert_eq!(deser.code_hash, original.code_hash);
        assert!(deser.account_id.is_none(), "account_id must drop");
        assert!(deser.code.is_none(), "code must drop");
    }

    #[test]
    fn block_env_round_trip_matrix() {
        // Cover every (prevrandao Some/None) × (blob_excess Some/None) combo,
        // each paired with a different field-value profile so we also exercise
        // ZERO / typical / MAX edges in a single dense pass.
        let cases = [
            // Some/Some: typical mainnet shape.
            BlockEnv {
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
                slot_num: 7,
            },
            // Some/None: prevrandao set, no blob fields. Pushes integer fields to MAX.
            BlockEnv {
                number: U256::MAX,
                beneficiary: Address::from([0xFE; 20]),
                timestamp: U256::from(u64::MAX),
                gas_limit: u64::MAX,
                basefee: u64::MAX,
                difficulty: U256::MAX,
                prevrandao: Some(B256::ZERO),
                blob_excess_gas_and_price: None,
                slot_num: u64::MAX,
            },
            // None/Some: covers the MAX `blob_gasprice` u128 path through the
            // [u8; 16] big-endian encoding.
            BlockEnv {
                number: U256::from(1u64),
                beneficiary: Address::ZERO,
                timestamp: U256::from(1u64),
                gas_limit: 1,
                basefee: 1,
                difficulty: U256::from(1u64),
                prevrandao: None,
                blob_excess_gas_and_price: Some(BlobExcessGasAndPrice {
                    excess_blob_gas: u64::MAX,
                    blob_gasprice: u128::MAX,
                }),
                slot_num: 1,
            },
            // None/None: ZERO baseline.
            BlockEnv {
                number: U256::ZERO,
                beneficiary: Address::ZERO,
                timestamp: U256::ZERO,
                gas_limit: 0,
                basefee: 0,
                difficulty: U256::ZERO,
                prevrandao: None,
                blob_excess_gas_and_price: None,
                slot_num: 0,
            },
        ];
        for env in &cases {
            let bytes = to_bytes_with!(BlockEnvRkyv, env);
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
            assert_eq!(deser.slot_num, env.slot_num);
        }
    }

    #[test]
    fn op_block_execution_ctx_round_trip_matrix() {
        // (parent_beacon_block_root Some/None) × (extra_data empty/small/larger).
        let cases = [
            OpBlockExecutionCtx {
                parent_hash: B256::repeat_byte(0xBB),
                parent_beacon_block_root: Some(B256::repeat_byte(0xCC)),
                extra_data: Bytes::from_static(&[1, 2, 3]),
                post_exec_mode: PostExecMode::Disabled,
            },
            OpBlockExecutionCtx {
                parent_hash: B256::ZERO,
                parent_beacon_block_root: None,
                extra_data: Bytes::new(),
                post_exec_mode: PostExecMode::Produce,
            },
            OpBlockExecutionCtx {
                parent_hash: B256::repeat_byte(0xFF),
                parent_beacon_block_root: Some(B256::ZERO),
                extra_data: Bytes::from_static(&[0; 64]),
                post_exec_mode: PostExecMode::Disabled,
            },
        ];
        for ctx in &cases {
            let bytes = to_bytes_with!(OpBlockExecutionCtxRkyv, ctx);
            let deser = from_bytes_with!(OpBlockExecutionCtxRkyv, OpBlockExecutionCtx, &bytes);
            assert_eq!(deser.parent_hash, ctx.parent_hash);
            assert_eq!(deser.parent_beacon_block_root, ctx.parent_beacon_block_root);
            assert_eq!(deser.extra_data, ctx.extra_data);
            assert_eq!(
                std::mem::discriminant(&deser.post_exec_mode),
                std::mem::discriminant(&ctx.post_exec_mode)
            );
        }
    }

    // -- AccountInfoRkyv --

    #[test]
    fn account_info_round_trip_matrix() {
        // Spans every (account_id Some/None) × (code Some/None) combination
        // alongside ZERO / typical / MAX value edges. The shared assertion
        // also checks the central drop invariant on every case.
        let cases = [
            // Some/Some + typical values: confirms transient fields drop even
            // when both are populated.
            info_with(11, U256::from(1u64 << 60), B256::repeat_byte(0xDE)),
            // None/None + ZERO baseline.
            AccountInfo {
                nonce: 0,
                balance: U256::ZERO,
                code_hash: B256::ZERO,
                account_id: None,
                code: None,
            },
            // Some/None + MAX values.
            AccountInfo {
                nonce: u64::MAX,
                balance: U256::MAX,
                code_hash: B256::repeat_byte(0xFF),
                account_id: Some(usize::MAX),
                code: None,
            },
            // None/Some: code present without an id.
            AccountInfo {
                nonce: 1,
                balance: U256::from(1u64),
                code_hash: B256::repeat_byte(0xAB),
                account_id: None,
                code: Some(Bytecode::new_raw(Bytes::from_static(&[0xFE]))),
            },
        ];
        for info in &cases {
            let bytes = to_bytes_with!(AccountInfoRkyv, info);
            let deser = from_bytes_with!(AccountInfoRkyv, AccountInfo, &bytes);
            assert_account_info_round_trip(&deser, info);
        }
    }

    // -- AccountStatusRkyv --

    #[test]
    fn account_status_round_trip_preserves_bits() {
        // empty(), single bit, multi-bit, and all-bits patterns
        let cases = [
            AccountStatus::empty(),
            AccountStatus::Created,
            AccountStatus::Touched | AccountStatus::Created | AccountStatus::Cold,
            AccountStatus::from_bits_retain(0xFF),
        ];
        for status in cases {
            let bytes = to_bytes_with!(AccountStatusRkyv, &status);
            let deser = from_bytes_with!(AccountStatusRkyv, AccountStatus, &bytes);
            assert_eq!(deser.bits(), status.bits(), "round trip {status:?}");
        }
    }

    // -- EvmStorageSlotRkyv --

    #[test]
    fn evm_storage_slot_round_trip_matrix() {
        // Persistent values are exercised over (typical, ZERO, MAX); transient
        // fields are seeded with both states (true / false) — both must drop on
        // round-trip regardless of input.
        let cases = [
            // typical + transient set
            slot(0xAA, 0xBB),
            // ZERO + transient cleared
            EvmStorageSlot {
                original_value: U256::ZERO,
                present_value: U256::ZERO,
                transaction_id: 0,
                is_cold: false,
            },
            // MAX original / 1 present + MAX transient_id, is_cold true
            EvmStorageSlot {
                original_value: U256::MAX,
                present_value: U256::from(1u64),
                transaction_id: usize::MAX,
                is_cold: true,
            },
        ];
        for s in &cases {
            let bytes = to_bytes_with!(EvmStorageSlotRkyv, s);
            let deser = from_bytes_with!(EvmStorageSlotRkyv, EvmStorageSlot, &bytes);
            assert_eq!(deser.original_value, s.original_value);
            assert_eq!(deser.present_value, s.present_value);
            // Transient fields always drop, regardless of input value.
            assert_eq!(deser.transaction_id, 0, "transaction_id must drop");
            assert!(!deser.is_cold, "is_cold must drop");
        }
    }

    // -- CacheStateRkyv --

    /// Single dense round-trip covering the full `CacheStateRkyv` surface in
    /// the same shape it sees in production: every `CacheAccountStatus` variant
    /// alongside multi-key storage, multiple contracts, and embedded
    /// `AccountInfo` edge values (ZERO, MAX, typical). Run once with
    /// `has_state_clear=true` and once with `false` so both branches encode and
    /// decode. A trailing fully-empty case exercises the empty-vector paths.
    #[test]
    fn cache_state_round_trip_dense() {
        let statuses = [
            (CacheAccountStatus::LoadedNotExisting, false),
            (CacheAccountStatus::Loaded, true),
            (CacheAccountStatus::LoadedEmptyEIP161, true),
            (CacheAccountStatus::InMemoryChange, true),
            (CacheAccountStatus::Changed, true),
            (CacheAccountStatus::Destroyed, true),
            (CacheAccountStatus::DestroyedChanged, true),
            (CacheAccountStatus::DestroyedAgain, true),
        ];

        {
            let mut cs = CacheState::new();
            for (i, (status, has_account)) in statuses.iter().enumerate() {
                let addr = Address::from([i as u8 + 1; 20]);
                // Mix AccountInfo edge cases inline so the embedded
                // AccountInfoRkyv path also sees ZERO and MAX values.
                let info = match i {
                    0 => AccountInfo {
                        nonce: 0,
                        balance: U256::ZERO,
                        code_hash: B256::ZERO,
                        account_id: None,
                        code: None,
                    },
                    1 => AccountInfo {
                        nonce: u64::MAX,
                        balance: U256::MAX,
                        code_hash: B256::repeat_byte(0xFF),
                        account_id: Some(usize::MAX),
                        code: None,
                    },
                    _ => info_with(
                        i as u64,
                        U256::from(1000 + i as u64),
                        B256::repeat_byte(i as u8),
                    ),
                };
                let account = has_account.then(|| {
                    let mut p = PlainAccount {
                        info,
                        storage: Default::default(),
                    };
                    p.storage
                        .insert(U256::from(i as u64 + 100), U256::from(i as u64 + 200));
                    p.storage.insert(U256::from(0xFFu64), U256::from(0xEEu64));
                    p
                });
                cs.accounts.insert(
                    addr,
                    CacheAccount {
                        account,
                        status: *status,
                    },
                );
            }
            // Multiple contracts with distinct hashes / payload sizes.
            cs.contracts.insert(
                B256::repeat_byte(0x01),
                Bytecode::new_raw(Bytes::from_static(&[0x60, 0x01])),
            );
            cs.contracts.insert(
                B256::repeat_byte(0x02),
                Bytecode::new_raw(Bytes::from_static(&[
                    0x60, 0x02, 0x60, 0x03, 0x60, 0x04, 0x60, 0x05,
                ])),
            );

            let bytes = to_bytes_with!(CacheStateRkyv, &cs);
            let deser = from_bytes_with!(CacheStateRkyv, CacheState, &bytes);

            assert_eq!(deser.accounts.len(), cs.accounts.len());
            assert_eq!(deser.contracts.len(), cs.contracts.len());

            for (addr, orig) in &cs.accounts {
                let got = deser.accounts.get(addr).expect("addr present");
                assert_eq!(got.status, orig.status, "status for {addr}");
                match (&got.account, &orig.account) {
                    (None, None) => {}
                    (Some(g), Some(o)) => {
                        assert_account_info_round_trip(&g.info, &o.info);
                        assert_eq!(g.storage, o.storage);
                    }
                    _ => panic!("account-presence mismatch for {addr}"),
                }
            }
            for (hash, orig) in &cs.contracts {
                let got = deser.contracts.get(hash).expect("hash present");
                assert_eq!(got.original_bytes(), orig.original_bytes());
            }
        }

        // Fully empty: exercises the empty-vector paths in both directions.
        let cs = CacheState::new();
        let bytes = to_bytes_with!(CacheStateRkyv, &cs);
        let deser = from_bytes_with!(CacheStateRkyv, CacheState, &bytes);
        assert!(deser.accounts.is_empty());
        assert!(deser.contracts.is_empty());
    }

    // -- ExecutionResultRkyv --

    fn all_oog_variants() -> [OutOfGasError; 6] {
        [
            OutOfGasError::Basic,
            OutOfGasError::MemoryLimit,
            OutOfGasError::Memory,
            OutOfGasError::Precompile,
            OutOfGasError::InvalidOperand,
            OutOfGasError::ReentrancySentry,
        ]
    }

    fn all_halt_reasons() -> Vec<HaltReason> {
        let mut v: Vec<HaltReason> = all_oog_variants()
            .into_iter()
            .map(HaltReason::OutOfGas)
            .collect();
        v.extend([
            HaltReason::OpcodeNotFound,
            HaltReason::InvalidFEOpcode,
            HaltReason::InvalidJump,
            HaltReason::NotActivated,
            HaltReason::StackUnderflow,
            HaltReason::StackOverflow,
            HaltReason::OutOfOffset,
            HaltReason::CreateCollision,
            HaltReason::PrecompileError,
            HaltReason::PrecompileErrorWithContext("ctx".into()),
            HaltReason::NonceOverflow,
            HaltReason::CreateContractSizeLimit,
            HaltReason::CreateContractStartingWithEF,
            HaltReason::CreateInitCodeSizeLimit,
            HaltReason::OverflowPayment,
            HaltReason::StateChangeDuringStaticCall,
            HaltReason::CallNotAllowedInsideStatic,
            HaltReason::OutOfFunds,
            HaltReason::CallTooDeep,
        ]);
        v
    }

    /// Single dense round-trip across every `ExecutionResult` shape:
    ///   Success  : SuccessReason × Output(Call/Create-with-addr/Create-no-addr,
    ///              empty / non-empty payload) × log counts {0, 1, 3}
    ///   Revert   : empty + non-empty output, ZERO and MAX gas
    ///   Halt     : every base `HaltReason` variant (incl. all `OutOfGasError`
    ///              and `PrecompileErrorWithContext`) plus `OpHaltReason::FailedDeposit`
    /// Folding these into one body is intentional: the variants share encoding
    /// machinery (`success_reason_byte`, `halt_reason_rkyv`, etc.) and a
    /// regression in any helper trips multiple branches at once.
    #[test]
    fn execution_result_round_trip_dense() {
        // -- Success --
        let log = Log::new_unchecked(
            Address::from([0xAB; 20]),
            vec![B256::repeat_byte(0x01), B256::repeat_byte(0x02)],
            Bytes::from_static(&[0xDE, 0xAD]),
        );
        let outputs = [
            Output::Call(Bytes::from_static(&[1, 2, 3])),
            Output::Call(Bytes::new()),
            Output::Create(
                Bytes::from_static(&[0x60, 0x80]),
                Some(Address::from([0xCD; 20])),
            ),
            Output::Create(Bytes::new(), None),
        ];
        for reason in [
            SuccessReason::Stop,
            SuccessReason::Return,
            SuccessReason::SelfDestruct,
        ] {
            for output in &outputs {
                for &count in &[0usize, 1, 3] {
                    let logs = vec![log.clone(); count];
                    // Seed every ResultGas component so all four survive the
                    // (total, state, refunded, floor) round-trip.
                    let gas = ResultGas::default()
                        .with_total_gas_spent(21_000)
                        .with_state_gas_spent(18_000)
                        .with_refunded(1_234)
                        .with_floor_gas(500);
                    let r = ExecutionResult::<OpHaltReason>::Success {
                        reason,
                        gas,
                        logs,
                        output: output.clone(),
                    };
                    let bytes = to_bytes_with!(ExecutionResultRkyv, &r);
                    let deser = from_bytes_with!(
                        ExecutionResultRkyv,
                        ExecutionResult<OpHaltReason>,
                        &bytes
                    );
                    let ExecutionResult::Success {
                        reason: deser_reason,
                        gas,
                        logs: deser_logs,
                        output: deser_output,
                    } = deser
                    else {
                        panic!("expected Success for reason={reason:?} output={output:?}");
                    };
                    assert_eq!(deser_reason, reason);
                    assert_eq!(gas.total_gas_spent(), 21_000);
                    assert_eq!(gas.state_gas_spent(), 18_000);
                    assert_eq!(gas.inner_refunded(), 1_234);
                    assert_eq!(gas.floor_gas(), 500);
                    assert_eq!(deser_logs.len(), count);
                    for got in &deser_logs {
                        assert_eq!(got.address, log.address);
                        assert_eq!(got.data.topics(), log.data.topics());
                        assert_eq!(got.data.data, log.data.data);
                    }
                    assert_eq!(&deser_output, output);
                }
            }
        }

        // -- Revert -- (empty / non-empty output, ZERO and MAX gas, logs present)
        for (total_gas, output) in [
            (500u64, Bytes::from_static(&[0xCA, 0xFE])),
            (0, Bytes::new()),
            (u64::MAX, Bytes::from_static(&[0xFF; 32])),
        ] {
            let r = ExecutionResult::<OpHaltReason>::Revert {
                gas: ResultGas::default().with_total_gas_spent(total_gas),
                logs: vec![log.clone()],
                output: output.clone(),
            };
            let bytes = to_bytes_with!(ExecutionResultRkyv, &r);
            let deser =
                from_bytes_with!(ExecutionResultRkyv, ExecutionResult<OpHaltReason>, &bytes);
            let ExecutionResult::Revert {
                gas: g,
                logs: revert_logs,
                output: o,
            } = deser
            else {
                panic!("expected Revert");
            };
            assert_eq!(g.total_gas_spent(), total_gas);
            assert_eq!(revert_logs.len(), 1);
            assert_eq!(o, output);
        }

        // -- Halt::Base -- every HaltReason
        for h in all_halt_reasons() {
            let r = ExecutionResult::<OpHaltReason>::Halt {
                reason: OpHaltReason::Base(h.clone()),
                gas: ResultGas::default().with_total_gas_spent(2_500),
                logs: vec![log.clone()],
            };
            let bytes = to_bytes_with!(ExecutionResultRkyv, &r);
            let deser =
                from_bytes_with!(ExecutionResultRkyv, ExecutionResult<OpHaltReason>, &bytes);
            let ExecutionResult::Halt {
                reason: OpHaltReason::Base(got),
                gas,
                logs: halt_logs,
            } = deser
            else {
                panic!("expected Halt::Base for {h:?}");
            };
            assert_eq!(got, h, "round trip {h:?}");
            assert_eq!(gas.total_gas_spent(), 2_500);
            assert_eq!(halt_logs.len(), 1);
        }

        // -- Halt::FailedDeposit --
        let r = ExecutionResult::<OpHaltReason>::Halt {
            reason: OpHaltReason::FailedDeposit,
            gas: ResultGas::default(),
            logs: vec![],
        };
        let bytes = to_bytes_with!(ExecutionResultRkyv, &r);
        let deser = from_bytes_with!(ExecutionResultRkyv, ExecutionResult<OpHaltReason>, &bytes);
        let ExecutionResult::Halt {
            reason: OpHaltReason::FailedDeposit,
            gas,
            logs: halt_logs,
        } = deser
        else {
            panic!("expected Halt::FailedDeposit");
        };
        assert_eq!(gas.total_gas_spent(), 0);
        assert!(halt_logs.is_empty());
    }

    // -- Panic paths for invalid discriminants --

    #[test]
    #[should_panic(expected = "invalid SuccessReason byte")]
    fn success_reason_from_byte_panics_on_invalid() {
        let _ = success_reason_from_byte(99);
    }

    #[test]
    #[should_panic(expected = "invalid OutOfGasError byte")]
    fn oog_from_byte_panics_on_invalid() {
        let _ = oog_from_byte(99);
    }

    #[test]
    #[should_panic(expected = "invalid CacheAccountStatus byte")]
    fn cache_account_status_from_byte_panics_on_invalid() {
        let _ = cache_account_status_from_byte(99);
    }

    #[test]
    #[should_panic(expected = "invalid HaltReason discriminant")]
    fn halt_reason_raw_panics_on_invalid_discriminant() {
        let _ = halt_reason_raw((99, 0, None));
    }

    #[test]
    #[should_panic(expected = "invalid OpHaltReason discriminant")]
    fn op_halt_reason_raw_panics_on_invalid_discriminant() {
        let _ = op_halt_reason_raw((99, None));
    }

    #[test]
    #[should_panic(expected = "invalid ExecutionResult discriminant")]
    fn execution_result_raw_panics_on_invalid_discriminant() {
        let _ = ExecutionResultRkyv::raw((99, 0, (0, 0, 0, 0), vec![], None, None, None));
    }

    #[test]
    #[should_panic(expected = "invalid Output discriminant")]
    fn execution_result_raw_panics_on_invalid_output_disc() {
        // Success disc with a malformed inner Output disc (only 0/1 are valid).
        let _ = ExecutionResultRkyv::raw((
            0,
            0,
            (0, 0, 0, 0),
            vec![],
            Some((9, vec![], None)),
            None,
            None,
        ));
    }

    #[test]
    #[should_panic(expected = "PostExecMode::Verify is unsupported")]
    fn post_exec_mode_byte_panics_on_verify() {
        let _ = post_exec_mode_byte(&PostExecMode::Verify(Default::default()));
    }

    #[test]
    #[should_panic(expected = "invalid PostExecMode byte")]
    fn post_exec_mode_from_byte_panics_on_invalid() {
        let _ = post_exec_mode_from_byte(99);
    }
}

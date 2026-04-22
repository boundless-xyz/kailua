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

use alloy_evm::op_revm::OpHaltReason;
use alloy_evm::revm::context::BlockEnv;
use alloy_evm::revm::context_interface::result::{
    ExecutionResult, HaltReason, OutOfGasError, Output, ResultAndState, SuccessReason,
};
use alloy_evm::revm::database::in_memory_db::{AccountState, Cache, DbAccount};
use alloy_evm::revm::primitives::KECCAK_EMPTY;
use alloy_evm::revm::state::{Account, AccountInfo, AccountStatus, EvmState};
use alloy_op_evm::block::OpBlockExecutionCtx;
use alloy_primitives::{Address, B256, U256};
use risc0_zkvm::sha::rust_crypto::{Digest, Sha256};
use std::collections::BTreeMap;

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
///
/// EIP-161 state-clearing rule: any account that is `Touched` and has empty info
/// (balance == 0, nonce == 0, no code) is removed from state, regardless of whether
/// it previously existed or was loaded as not existing. This applies uniformly to
/// both absent accounts (zero-value call to new address) and existing-but-empty
/// accounts (e.g. drained all balance).
pub fn account_state_from_evm_status(status: AccountStatus, info: &AccountInfo) -> AccountState {
    if status.intersects(AccountStatus::SelfDestructed | AccountStatus::Created) {
        AccountState::StorageCleared
    } else if status.contains(AccountStatus::Touched) {
        if info.is_empty() {
            // EIP-161: touched empty accounts are removed from state.
            AccountState::NotExisting
        } else {
            AccountState::Touched
        }
    } else if status.contains(AccountStatus::LoadedAsNotExisting) {
        // Read-only access to absent account (no Touched flag).
        AccountState::NotExisting
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
/// state delta from a [`PartialExecution`] proof to advance state during aggregation).
///
/// # `KECCAK_EMPTY` edge case (review finding L-2)
///
/// Contract bytecode is inserted into `cache.contracts` only when `code_hash !=
/// KECCAK_EMPTY` AND `account.info.code.is_some()`. If a witness-supplied trace has
/// `code_hash != KECCAK_EMPTY` but `code.is_none()` (a legal but unusual combination
/// in revm's state model), no bytecode is inserted. Subsequent reads against
/// `CacheDB<PanicDB>` for that code_hash would panic in the chunk guest. In the
/// aggregation path where the backing DB is `State<TrieDB>`, code is lazily fetched
/// through the preimage oracle by code_hash, so the outcome is the same as honest
/// execution. `validate_cached_contracts` (called at chunk entry) only checks pairs
/// present in `cache.contracts` — it cannot flag a missing entry for a present
/// `code_hash`. This is mitigated transitively by the chunk proof's `results_hash`
/// binding: the chunk guest executed with whatever bytecode state was in its
/// witness, and any divergence between aggregation's lazy-loaded bytecode and the
/// chunk guest's bytecode would produce different state transitions, failing the
/// final state-root check.
pub fn apply_trace_to_cache(cache: &mut Cache, trace: &EvmState) {
    for (addr, account) in trace {
        let db_account = cache.accounts.entry(*addr).or_insert_with(|| DbAccount {
            info: AccountInfo::default(),
            account_state: AccountState::NotExisting,
            storage: Default::default(),
        });

        db_account.info = account.info.clone();
        db_account.account_state = account_state_from_evm_status(account.status, &account.info);

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

/// Compute the chunk_trace commitment from the two input hashes.
///
/// Returns:
/// ```text
/// SHA256(results_hash || block_ctx_hash)
/// ```
pub fn compute_chunk_trace(results_hash: B256, block_ctx_hash: B256) -> B256 {
    let mut hasher = Sha256::new();
    hasher.update(results_hash.as_slice());
    hasher.update(block_ctx_hash.as_slice());
    B256::from_slice(&hasher.finalize())
}

/// Canonical SHA256 of the block execution context (`BlockEnv` + `OpBlockExecutionCtx`).
///
/// Committed into `chunk_trace` so the chunk proof authenticates the exact block
/// context under which its transactions executed. Any env-sensitive opcode
/// (BASEFEE, PREVRANDAO, NUMBER, COINBASE, TIMESTAMP, BLOBBASEFEE, BLOCKHASH, the
/// EIP-4788 beacon-root system call, EIP-2935 parent-hash ring, Holocene/Jovian
/// EIP-1559 params encoded in extra_data) reads from these fields; a forged context
/// would produce different results, so the chunk proof must carry them.
///
/// Encoding (streamed into SHA256):
///
/// ```text
/// BlockEnv:
///     u256_be(number)
///     address(beneficiary)                  20 bytes
///     u256_be(timestamp)
///     u64_be(gas_limit)
///     u64_be(basefee)
///     u256_be(difficulty)
///     option(prevrandao) -> 1-byte tag + 32-byte B256 if Some
///     option(blob_excess_gas_and_price) -> 1-byte tag + u64_be(excess_blob_gas) + u128_be(blob_gasprice)
///
/// OpBlockExecutionCtx:
///     b256(parent_hash)                     32 bytes
///     option(parent_beacon_block_root) -> 1-byte tag + 32-byte B256 if Some
///     u64_be(len(extra_data)) || extra_data bytes
/// ```
///
/// This encoding matches the `BlockEnvRkyv` / `OpBlockExecutionCtxRkyv` field set
/// (see `rkyv/chunking.rs`) but uses a canonical byte stream instead of the rkyv
/// archived layout, so hash equality is independent of archive representation.
pub fn hash_block_ctx(block_env: &BlockEnv, op_block_ctx: &OpBlockExecutionCtx) -> B256 {
    let mut hasher = Sha256::new();

    // --- BlockEnv ---
    write_u256(&mut hasher, &block_env.number);
    write_address(&mut hasher, &block_env.beneficiary);
    write_u256(&mut hasher, &block_env.timestamp);
    write_u64(&mut hasher, block_env.gas_limit);
    write_u64(&mut hasher, block_env.basefee);
    write_u256(&mut hasher, &block_env.difficulty);
    match &block_env.prevrandao {
        Some(r) => {
            hasher.update([1u8]);
            write_b256(&mut hasher, r);
        }
        None => hasher.update([0u8]),
    }
    match &block_env.blob_excess_gas_and_price {
        Some(b) => {
            hasher.update([1u8]);
            write_u64(&mut hasher, b.excess_blob_gas);
            hasher.update(b.blob_gasprice.to_be_bytes());
        }
        None => hasher.update([0u8]),
    }

    // --- OpBlockExecutionCtx ---
    write_b256(&mut hasher, &op_block_ctx.parent_hash);
    match &op_block_ctx.parent_beacon_block_root {
        Some(r) => {
            hasher.update([1u8]);
            write_b256(&mut hasher, r);
        }
        None => hasher.update([0u8]),
    }
    write_bytes(&mut hasher, &op_block_ctx.extra_data);

    B256::from_slice(&hasher.finalize())
}

// ============================================================================
//  hash_results — canonical SHA256 of `Vec<ResultAndState<OpHaltReason>>`
// ============================================================================
//
// Canonical streaming SHA256 encoding of a per-transaction execution trace.
//
// This hash is committed into `chunk_trace` so that the chunk proof authenticates not
// just the pre→post state hashes but the *exact sequence* of per-transaction
// `ResultAndState` entries produced during execution. This prevents a class of
// aggregation-side forgeries where an adversary could replay different `results` that
// coincidentally take `pre_db_hash` to `post_db_hash`.
//
// Encoding (streamed into the SHA256 hasher in this exact order):
//
// ```text
// u64_be(len(results))
// for each result in order:
//     encode_execution_result(result.result)
//     encode_evm_state(result.state)
// ```
//
// The encoding deliberately excludes transient revm fields (`transaction_id`, `is_cold`,
// `Account.original_info`, `AccountInfo.code`) — these either vary by execution context
// without affecting state correctness or are redundant with `code_hash`. This matches
// the exclusion pattern in `AccountRkyv` (see `rkyv/chunking.rs`).
//
// All length prefixes use `u64` big-endian. All 256-bit values (U256, B256) are encoded
// in 32 bytes big-endian. Addresses are 20 raw bytes. Enums use explicit discriminant
// bytes starting at 0 (see variant tables below).

fn write_u64(hasher: &mut Sha256, v: u64) {
    hasher.update(v.to_be_bytes());
}

fn write_bytes(hasher: &mut Sha256, data: &[u8]) {
    write_u64(hasher, data.len() as u64);
    hasher.update(data);
}

fn write_u256(hasher: &mut Sha256, v: &U256) {
    hasher.update(v.to_be_bytes::<32>());
}

fn write_b256(hasher: &mut Sha256, v: &B256) {
    hasher.update(v.0);
}

fn write_address(hasher: &mut Sha256, addr: &Address) {
    hasher.update(addr.0 .0);
}

/// Discriminants for `SuccessReason`. Matches the ordering in `rkyv/chunking.rs`
/// (`success_reason_byte`) so both encodings stay in sync.
fn success_reason_disc(r: &SuccessReason) -> u8 {
    match r {
        SuccessReason::Stop => 0,
        SuccessReason::Return => 1,
        SuccessReason::SelfDestruct => 2,
    }
}

/// Discriminants for `OutOfGasError`. Matches `rkyv/chunking.rs::oog_byte`.
fn oog_disc(e: &OutOfGasError) -> u8 {
    match e {
        OutOfGasError::Basic => 0,
        OutOfGasError::MemoryLimit => 1,
        OutOfGasError::Memory => 2,
        OutOfGasError::Precompile => 3,
        OutOfGasError::InvalidOperand => 4,
        OutOfGasError::ReentrancySentry => 5,
    }
}

/// Discriminants for `HaltReason`. Matches `rkyv/chunking.rs::halt_reason_rkyv`.
fn write_halt_reason(hasher: &mut Sha256, r: &HaltReason) {
    match r {
        HaltReason::OutOfGas(e) => {
            hasher.update([0, oog_disc(e)]);
        }
        HaltReason::OpcodeNotFound => hasher.update([1, 0]),
        HaltReason::InvalidFEOpcode => hasher.update([2, 0]),
        HaltReason::InvalidJump => hasher.update([3, 0]),
        HaltReason::NotActivated => hasher.update([4, 0]),
        HaltReason::StackUnderflow => hasher.update([5, 0]),
        HaltReason::StackOverflow => hasher.update([6, 0]),
        HaltReason::OutOfOffset => hasher.update([7, 0]),
        HaltReason::CreateCollision => hasher.update([8, 0]),
        HaltReason::PrecompileError => hasher.update([9, 0]),
        HaltReason::PrecompileErrorWithContext(s) => {
            hasher.update([10u8, 0]);
            write_bytes(hasher, s.as_bytes());
        }
        HaltReason::NonceOverflow => hasher.update([11, 0]),
        HaltReason::CreateContractSizeLimit => hasher.update([12, 0]),
        HaltReason::CreateContractStartingWithEF => hasher.update([13, 0]),
        HaltReason::CreateInitCodeSizeLimit => hasher.update([14, 0]),
        HaltReason::OverflowPayment => hasher.update([15, 0]),
        HaltReason::StateChangeDuringStaticCall => hasher.update([16, 0]),
        HaltReason::CallNotAllowedInsideStatic => hasher.update([17, 0]),
        HaltReason::OutOfFunds => hasher.update([18, 0]),
        HaltReason::CallTooDeep => hasher.update([19, 0]),
    }
}

fn write_op_halt_reason(hasher: &mut Sha256, r: &OpHaltReason) {
    match r {
        OpHaltReason::Base(h) => {
            hasher.update([0u8]);
            write_halt_reason(hasher, h);
        }
        OpHaltReason::FailedDeposit => {
            hasher.update([1u8]);
        }
    }
}

fn write_logs(hasher: &mut Sha256, logs: &[alloy_primitives::Log]) {
    write_u64(hasher, logs.len() as u64);
    for log in logs {
        write_address(hasher, &log.address);
        let topics = log.data.topics();
        write_u64(hasher, topics.len() as u64);
        for t in topics {
            write_b256(hasher, t);
        }
        write_bytes(hasher, &log.data.data);
    }
}

fn write_output(hasher: &mut Sha256, output: &Output) {
    match output {
        Output::Call(data) => {
            hasher.update([0u8]);
            write_bytes(hasher, data);
        }
        Output::Create(data, addr_opt) => {
            hasher.update([1u8]);
            write_bytes(hasher, data);
            match addr_opt {
                Some(addr) => {
                    hasher.update([1u8]);
                    write_address(hasher, addr);
                }
                None => hasher.update([0u8]),
            }
        }
    }
}

fn write_execution_result(hasher: &mut Sha256, r: &ExecutionResult<OpHaltReason>) {
    match r {
        ExecutionResult::Success {
            reason,
            gas_used,
            gas_refunded,
            logs,
            output,
        } => {
            hasher.update([0u8, success_reason_disc(reason)]);
            write_u64(hasher, *gas_used);
            write_u64(hasher, *gas_refunded);
            write_logs(hasher, logs);
            write_output(hasher, output);
        }
        ExecutionResult::Revert { gas_used, output } => {
            hasher.update([1u8]);
            write_u64(hasher, *gas_used);
            write_bytes(hasher, output);
        }
        ExecutionResult::Halt { reason, gas_used } => {
            hasher.update([2u8]);
            write_op_halt_reason(hasher, reason);
            write_u64(hasher, *gas_used);
        }
    }
}

/// Encode a single `Account`. Skips transient fields (`original_info`,
/// `transaction_id`) and the redundant `AccountInfo.code` (committed via `code_hash`).
/// `EvmStorageSlot` encodes both `original_value` and `present_value` (the storage
/// transition) but skips `transaction_id` and `is_cold`.
///
/// # Upgrade hazard (review finding M-6): `AccountStatus::bits()` width
///
/// `acct.status.bits()` returns `u8` — the current revm `AccountStatus` bitflag fits
/// in 8 bits (Cold, LoadedAsNotExisting, Touched, Created, CreatedLocal,
/// SelfDestructed, SelfDestructedLocal, plus one reserve). If a future revm release
/// widens this to `u16`, `.bits()` returns `u16` and our `[byte]` call would fail to
/// compile (good — forces an explicit upgrade decision). If instead revm swaps the
/// underlying type silently (unlikely), the static assertion below traps the change.
const _: () = {
    // Compile-time assertion: AccountStatus.bits() must be u8.
    let _check: u8 = 0u8.wrapping_add(0);
    let _: fn() -> u8 = || {
        let s = AccountStatus::empty();
        s.bits()
    };
};

fn write_account(hasher: &mut Sha256, acct: &Account) {
    // Pre-tx AccountInfo (original_info) — revm's first-load value for this address.
    // Authenticated so CachedEvm's serve-side `db.basic(addr)` check against
    // `account.original_info` is bound by the chunk proof.
    write_u64(hasher, acct.original_info.nonce);
    write_u256(hasher, &acct.original_info.balance);
    write_b256(hasher, &acct.original_info.code_hash);

    // Post-tx AccountInfo (info): nonce, balance, code_hash. Skip `account_id` and
    // `code` field (code_hash binds bytecode content via `validate_cached_contracts`).
    write_u64(hasher, acct.info.nonce);
    write_u256(hasher, &acct.info.balance);
    write_b256(hasher, &acct.info.code_hash);

    // Status bitflags (u8 — see M-6 assertion above).
    hasher.update([acct.status.bits()]);

    // Storage, sorted by slot key.
    let mut entries: Vec<_> = acct.storage.iter().collect();
    entries.sort_by_key(|(k, _)| *k);
    write_u64(hasher, entries.len() as u64);
    for (slot, evm_slot) in entries {
        write_u256(hasher, slot);
        write_u256(hasher, &evm_slot.original_value);
        write_u256(hasher, &evm_slot.present_value);
    }
}

/// Encode an `EvmState` (the per-tx state diff). Entries are sorted by address so the
/// encoding is invariant to the underlying `HashMap` iteration order.
fn write_evm_state(hasher: &mut Sha256, state: &EvmState) {
    let mut entries: Vec<_> = state.iter().collect();
    entries.sort_by_key(|(addr, _)| *addr);
    write_u64(hasher, entries.len() as u64);
    for (addr, account) in entries {
        write_address(hasher, addr);
        write_account(hasher, account);
    }
}

/// Canonical SHA256 of a list of txn hashes and their `Vec<ResultAndState<OpHaltReason>>`
pub fn hash_results(tx_hashes: &[B256], results: &[ResultAndState<OpHaltReason>]) -> B256 {
    assert_eq!(
        tx_hashes.len(),
        results.len(),
        "hash_results: tx_hashes and results must have the same length",
    );
    let mut hasher = Sha256::new();
    write_u64(&mut hasher, results.len() as u64);
    for (tx_hash, ras) in tx_hashes.iter().zip(results) {
        hasher.update(tx_hash.as_slice());
        write_execution_result(&mut hasher, &ras.result);
        write_evm_state(&mut hasher, &ras.state);
    }
    B256::from_slice(&hasher.finalize())
}

use alloy_consensus::{BlockHeader, Header};
use alloy_eips::eip7840::BlobParams;
use alloy_evm::op_revm::OpSpecId;
use alloy_evm::revm::context_interface::block::BlobExcessGasAndPrice;
use alloy_evm::revm::primitives::eip4844::{
    BLOB_BASE_FEE_UPDATE_FRACTION_CANCUN, BLOB_BASE_FEE_UPDATE_FRACTION_PRAGUE,
};

/// Compute the `BlobExcessGasAndPrice` that kona's `prepare_block_env` would assign
/// to `BlockEnv.blob_excess_gas_and_price` for a block with the given parent header
/// and spec_id. Mirrors `kona/proof/executor/src/builder/env.rs::prepare_block_env`
/// exactly — if that implementation changes in upstream, this must too.
pub fn expected_blob_excess_gas_and_price(
    parent_header: &Header,
    spec_id: OpSpecId,
) -> Option<BlobExcessGasAndPrice> {
    let (params, fraction) = if spec_id.is_enabled_in(OpSpecId::ISTHMUS) {
        (
            Some(BlobParams::prague()),
            BLOB_BASE_FEE_UPDATE_FRACTION_PRAGUE,
        )
    } else if spec_id.is_enabled_in(OpSpecId::ECOTONE) {
        (
            Some(BlobParams::cancun()),
            BLOB_BASE_FEE_UPDATE_FRACTION_CANCUN,
        )
    } else {
        (None, 0)
    };

    parent_header
        .maybe_next_block_excess_blob_gas(params)
        .or_else(|| spec_id.is_enabled_in(OpSpecId::ECOTONE).then_some(0))
        .map(|excess| BlobExcessGasAndPrice::new(excess, fraction))
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use alloy_evm::revm::database::in_memory_db::AccountState;
    use alloy_evm::revm::primitives::HashMap;
    use alloy_evm::revm::state::Bytecode;
    use alloy_primitives::{address, U256};
    use risc0_zkvm::sha::Digestible;

    fn make_info(nonce: u64, balance: u64) -> AccountInfo {
        AccountInfo {
            nonce,
            balance: U256::from(balance),
            code_hash: B256::ZERO,
            account_id: None,
            code: None,
        }
    }

    pub fn make_bytecode(bytes: &'static [u8]) -> Bytecode {
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

    // ========== compute_chunk_trace tests ==========

    #[test]
    fn chunk_trace_deterministic() {
        let a = B256::repeat_byte(0x01);
        let b = B256::repeat_byte(0x02);
        let t1 = compute_chunk_trace(a, b);
        let t2 = compute_chunk_trace(a, b);
        assert_eq!(t1, t2);
        assert!(!t1.is_zero());
    }

    #[test]
    fn chunk_trace_any_input_change_different() {
        let base = [B256::repeat_byte(0x01), B256::repeat_byte(0x02)];
        let baseline = compute_chunk_trace(base[0], base[1]);
        for i in 0..2 {
            let mut modified = base;
            modified[i] = B256::repeat_byte(0xFF);
            let h = compute_chunk_trace(modified[0], modified[1]);
            assert_ne!(
                baseline, h,
                "changing input {i} should produce different trace"
            );
        }
    }

    // ========== hash_results tests (results ↔ chunk_trace binding) ==========

    fn stub_success(gas_used: u64) -> ResultAndState<OpHaltReason> {
        ResultAndState {
            result: ExecutionResult::Success {
                reason: SuccessReason::Return,
                gas_used,
                gas_refunded: 0,
                logs: vec![],
                output: Output::Call(alloy_primitives::Bytes::new()),
            },
            state: Default::default(),
        }
    }

    fn stub_revert(gas_used: u64, output: &[u8]) -> ResultAndState<OpHaltReason> {
        ResultAndState {
            result: ExecutionResult::Revert {
                gas_used,
                output: alloy_primitives::Bytes::copy_from_slice(output),
            },
            state: Default::default(),
        }
    }

    #[test]
    fn hash_results_empty_is_deterministic() {
        let h1 = hash_results(&[], &[]);
        let h2 = hash_results(&[], &[]);
        assert_eq!(h1, h2);
        assert!(!h1.is_zero(), "hash of empty trace is u64_be(0) hash");
    }

    #[test]
    fn hash_results_single_entry_deterministic() {
        let entry = stub_success(21000);
        let h1 = hash_results(&[B256::ZERO], std::slice::from_ref(&entry));
        let h2 = hash_results(&[B256::ZERO], std::slice::from_ref(&entry));
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_results_order_sensitive() {
        let a = stub_success(21000);
        let b = stub_success(42000);
        let h_ab = hash_results(&[B256::ZERO, B256::ZERO], &[a.clone(), b.clone()]);
        let h_ba = hash_results(&[B256::ZERO, B256::ZERO], &[b, a]);
        assert_ne!(
            h_ab, h_ba,
            "reordering results must change the hash (prevents aggregator-side reordering)"
        );
    }

    #[test]
    fn hash_results_gas_change_different() {
        let h1 = hash_results(&[B256::ZERO], &[stub_success(21000)]);
        let h2 = hash_results(&[B256::ZERO], &[stub_success(21001)]);
        assert_ne!(h1, h2);
    }

    #[test]
    fn hash_results_variant_change_different() {
        let success = hash_results(&[B256::ZERO], &[stub_success(21000)]);
        let revert = hash_results(&[B256::ZERO], &[stub_revert(21000, &[])]);
        assert_ne!(success, revert);
    }

    /// `original_info` (pre-tx `AccountInfo`) now contributes to the canonical hash —
    /// it's what `CachedEvm::transact_raw` authenticates against the live DB at
    /// serve time, so any tampering must invalidate the chunk journal.
    #[test]
    fn hash_results_original_info_contributes() {
        use alloy_evm::revm::state::EvmStorageSlot;
        let addr = Address::from([0xAA; 20]);
        let post_info = AccountInfo {
            nonce: 1,
            balance: U256::from(1000),
            code_hash: B256::ZERO,
            account_id: None,
            code: None,
        };
        let make_ras = |original: AccountInfo| {
            let mut ras = stub_success(21000);
            ras.state.insert(
                addr,
                Account {
                    info: post_info.clone(),
                    original_info: Box::new(original),
                    transaction_id: 0,
                    storage: HashMap::<U256, EvmStorageSlot>::default(),
                    status: alloy_evm::revm::state::AccountStatus::Touched,
                },
            );
            ras
        };
        let h_default = hash_results(
            &[B256::ZERO],
            std::slice::from_ref(&make_ras(AccountInfo::default())),
        );
        let h_nonzero = hash_results(
            &[B256::ZERO],
            std::slice::from_ref(&make_ras(AccountInfo {
                nonce: 5,
                balance: U256::from(999),
                code_hash: B256::ZERO,
                account_id: None,
                code: None,
            })),
        );
        assert_ne!(
            h_default, h_nonzero,
            "differing original_info must change the results hash"
        );
    }

    #[test]
    fn hash_results_state_change_different() {
        use alloy_evm::revm::state::EvmStorageSlot;
        let addr = Address::from([0xAA; 20]);

        // Baseline: empty state.
        let base = stub_success(21000);
        let h_base = hash_results(&[B256::ZERO], std::slice::from_ref(&base));

        // Same result but with one account added to state.
        let mut modified = stub_success(21000);
        modified.state.insert(
            addr,
            Account {
                info: AccountInfo {
                    nonce: 1,
                    balance: U256::from(1000),
                    code_hash: B256::ZERO,
                    account_id: None,
                    code: None,
                },
                original_info: Box::new(AccountInfo::default()),
                transaction_id: 0,
                storage: Default::default(),
                status: alloy_evm::revm::state::AccountStatus::Touched,
            },
        );
        let h_modified = hash_results(&[B256::ZERO], std::slice::from_ref(&modified));

        assert_ne!(
            h_base, h_modified,
            "state diff must contribute to results hash"
        );

        // Mutate a storage slot — hash must change.
        modified.state.get_mut(&addr).unwrap().storage.insert(
            U256::from(1),
            EvmStorageSlot::new_changed(U256::ZERO, U256::from(42), 0),
        );
        let h_with_storage = hash_results(&[B256::ZERO], std::slice::from_ref(&modified));
        assert_ne!(h_modified, h_with_storage);
    }

    /// Transient fields (`transaction_id`, `is_cold`) must not contribute to the
    /// hash. `original_info` now DOES contribute (authenticated pre-tx view), so
    /// this test keeps it identical across ras1/ras2.
    #[test]
    fn hash_results_ignores_transient_fields() {
        use alloy_evm::revm::state::EvmStorageSlot;
        let addr = Address::from([0xAA; 20]);

        let info = AccountInfo {
            nonce: 1,
            balance: U256::from(1000),
            code_hash: B256::ZERO,
            account_id: None,
            code: None,
        };
        let original_info = AccountInfo::default();

        let mut ras1 = stub_success(21000);
        ras1.state.insert(
            addr,
            Account {
                info: info.clone(),
                original_info: Box::new(original_info.clone()),
                transaction_id: 7,
                storage: {
                    let mut s: HashMap<U256, EvmStorageSlot> = Default::default();
                    s.insert(
                        U256::from(1),
                        EvmStorageSlot {
                            original_value: U256::ZERO,
                            present_value: U256::from(42),
                            transaction_id: 7,
                            is_cold: false,
                        },
                    );
                    s
                },
                status: alloy_evm::revm::state::AccountStatus::Touched,
            },
        );

        let mut ras2 = stub_success(21000);
        ras2.state.insert(
            addr,
            Account {
                info: info.clone(),
                original_info: Box::new(original_info.clone()),
                // Different transaction_id — must not affect hash.
                transaction_id: 99,
                storage: {
                    let mut s: HashMap<U256, EvmStorageSlot> = Default::default();
                    s.insert(
                        U256::from(1),
                        EvmStorageSlot {
                            original_value: U256::ZERO,
                            present_value: U256::from(42),
                            // Different transaction_id and is_cold — must not affect hash.
                            transaction_id: 99,
                            is_cold: true,
                        },
                    );
                    s
                },
                status: alloy_evm::revm::state::AccountStatus::Touched,
            },
        );

        assert_eq!(
            hash_results(&[B256::ZERO], std::slice::from_ref(&ras1)),
            hash_results(&[B256::ZERO], std::slice::from_ref(&ras2)),
            "transient fields must not affect the canonical hash"
        );
    }

    // ========== account_state_from_evm_status / apply_trace_to_cache tests ==========

    // The `account_state_from_evm_status` function takes the bitflag-based
    // `AccountStatus` from `revm-state` (used by `EvmState` trace `Account`),
    // NOT the enum-based `AccountStatus` from `revm-database::states`.
    // We alias the bitflag version to avoid shadowing the test module's enum import.
    use alloy_evm::revm::state::AccountStatus as EvmAccountStatus;

    #[test]
    fn loaded_as_not_existing_maps_to_not_existing() {
        // A read-only access to a non-existent account produces LoadedAsNotExisting
        // in EvmState traces. This must map to NotExisting.
        let empty = AccountInfo::default();
        assert_eq!(
            account_state_from_evm_status(EvmAccountStatus::LoadedAsNotExisting, &empty),
            AccountState::NotExisting,
        );
    }

    #[test]
    fn loaded_as_not_existing_touched_empty_maps_to_not_existing() {
        // A touched absent account with empty info (e.g. zero-value call to
        // non-existent address) is still NotExisting — EIP-161 cleanup.
        let empty = AccountInfo::default();
        assert_eq!(
            account_state_from_evm_status(
                EvmAccountStatus::LoadedAsNotExisting | EvmAccountStatus::Touched,
                &empty,
            ),
            AccountState::NotExisting,
        );
    }

    #[test]
    fn loaded_as_not_existing_touched_nonempty_maps_to_touched() {
        // A touched absent account that now has balance (e.g. received ETH)
        // is Touched — the account was effectively created via value transfer.
        let funded = make_info(0, 1000);
        assert_eq!(
            account_state_from_evm_status(
                EvmAccountStatus::LoadedAsNotExisting | EvmAccountStatus::Touched,
                &funded,
            ),
            AccountState::Touched,
        );
    }

    #[test]
    fn untouched_account_maps_to_none() {
        // An empty status (loaded existing account, not touched) maps to None.
        let empty = AccountInfo::default();
        assert_eq!(
            account_state_from_evm_status(EvmAccountStatus::empty(), &empty),
            AccountState::None,
        );
    }

    #[test]
    fn touched_nonempty_account_maps_to_touched() {
        let info = make_info(1, 100);
        assert_eq!(
            account_state_from_evm_status(EvmAccountStatus::Touched, &info),
            AccountState::Touched,
        );
    }

    #[test]
    fn touched_empty_existing_account_maps_to_not_existing() {
        // EIP-161: an existing account that is touched but ends up empty
        // (e.g. drained all balance) is removed from state.
        let empty = AccountInfo::default();
        assert_eq!(
            account_state_from_evm_status(EvmAccountStatus::Touched, &empty),
            AccountState::NotExisting,
        );
    }

    #[test]
    fn created_account_maps_to_storage_cleared() {
        let info = make_info(0, 0);
        assert_eq!(
            account_state_from_evm_status(
                EvmAccountStatus::Created | EvmAccountStatus::Touched,
                &info,
            ),
            AccountState::StorageCleared,
        );
    }

    #[test]
    fn selfdestructed_account_maps_to_storage_cleared() {
        let info = make_info(0, 0);
        assert_eq!(
            account_state_from_evm_status(
                EvmAccountStatus::SelfDestructed | EvmAccountStatus::Touched,
                &info,
            ),
            AccountState::StorageCleared,
        );
    }

    #[test]
    fn apply_trace_preserves_not_existing_for_absent_account_read() {
        // Regression test: when a chunk reads a non-existent account, the cumulative
        // cache must record it as NotExisting so the next chunk's pre_db_hash matches.
        use alloy_evm::revm::state::Account;

        let addr = address!("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef");
        let mut cache = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        // Pre-populate the address as NotExisting (as witness construction does)
        cache.accounts.insert(addr, DbAccount::new_not_existing());

        // Simulate a trace where the account was read but doesn't exist
        let account = Account::new_not_existing(0);
        let mut trace: EvmState = Default::default();
        trace.insert(addr, account);

        apply_trace_to_cache(&mut cache, &trace);

        // After applying the trace, the account must still be NotExisting
        assert_eq!(
            cache.accounts[&addr].account_state,
            AccountState::NotExisting,
            "absent account read must preserve NotExisting state for hash chain continuity"
        );
    }

    #[test]
    fn chunk_trace_integration_with_precondition() {
        let trace = compute_chunk_trace(B256::repeat_byte(0x01), B256::repeat_byte(0x02));
        let p = crate::precondition::Precondition::default().chunk(trace);
        assert_eq!(p.digest(), risc0_zkvm::Digest::from_bytes(trace.0));
    }

}

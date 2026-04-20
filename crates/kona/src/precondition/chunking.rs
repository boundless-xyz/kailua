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
use alloy_evm::revm::database::states::cache::CacheState;
use alloy_evm::revm::primitives::{HashMap, KECCAK_EMPTY};
use alloy_evm::revm::state::{Account, AccountInfo, AccountStatus, Bytecode, EvmState};
use alloy_op_evm::block::OpBlockExecutionCtx;
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
/// state delta from a [`Chunk`] proof to advance state during aggregation).
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

/// Validates that each cached contract bytecode hashes to its map key.
///
/// This is required because the canonical state hash encodes only the sorted set of contract
/// code-hash keys. If a witness provided arbitrary bytecode under a valid key and we failed to
/// check it first, the hash would authenticate the wrong executable code.
pub fn validate_cached_contracts<S: std::hash::BuildHasher>(
    contracts: &HashMap<B256, Bytecode, S>,
) {
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

/// Compute a deterministic hash of a transaction list.
/// Encoding: `SHA256(u64_be(len) || for each tx: u64_be(tx.len()) || tx_bytes)`.
/// This matches the length-prefix pattern used by the existing memdb hashing functions.
pub fn compute_tx_hash(transactions: &[Vec<u8>]) -> B256 {
    let mut hasher = Sha256::new();
    hasher.update((transactions.len() as u64).to_be_bytes());
    for tx in transactions {
        hasher.update((tx.len() as u64).to_be_bytes());
        hasher.update(tx);
    }
    B256::from_slice(&hasher.finalize())
}

/// Compute the chunk_trace commitment from the seven input hashes.
///
/// Returns:
/// ```text
/// SHA256(
///     tx_hash
///  || pre_db_hash
///  || post_db_hash
///  || pre_evm_hash
///  || post_evm_hash
///  || results_hash
///  || block_ctx_hash
/// )
/// ```
///
/// # Why each component matters
///
/// * `tx_hash` — binds the transaction sequence.
/// * `pre_db_hash` / `post_db_hash` — binds the pre and post flat DB state hashes.
/// * `pre_evm_hash` / `post_evm_hash` — binds the pre and post EVM accumulator hashes
///   (cumulative gas, DA footprint, blob gas, logs bloom, cumulative receipts).
/// * `results_hash` — binds the per-transaction `ResultAndState` trajectory so the
///   aggregation cannot substitute a different-but-endpoint-equivalent results vec.
///   Without this, the aggregator could replay arbitrary witness-supplied
///   `ResultAndState` entries that happen to take `pre_db_hash` → `post_db_hash`.
///   See `hash_results` for the canonical encoding.
/// * `block_ctx_hash` — binds the block execution context (BlockEnv + OpBlockExecutionCtx)
///   under which the chunk guest executed. Without this, an adversary could generate a
///   chunk proof with a forged `block_env` (different timestamp, basefee, prevrandao,
///   coinbase, blob pricing, etc.) or a forged `op_block_ctx` (different parent_hash,
///   parent_beacon_block_root, extra_data), producing different results for
///   env-sensitive opcodes (BASEFEE / PREVRANDAO / NUMBER / COINBASE / TIMESTAMP /
///   BLOBBASEFEE / BLOCKHASH for EIP-2935, beacon-root EIP-4788 call, Holocene/Jovian
///   EIP-1559 params encoded in extra_data), and have aggregation accept those results
///   for the real block. With `block_ctx_hash` folded in, the chunk proof's journal
///   identity depends on the exact context used; the aggregation side recomputes the
///   same hash from `chunk.block_env`/`chunk.op_block_ctx` and ALSO cross-checks
///   those carried fields against the derivation pipeline's header (see
///   `verify_block_chunks`), rejecting any chunk whose context doesn't match the
///   block being aggregated.
pub fn compute_chunk_trace(
    tx_hash: B256,
    pre_db_hash: B256,
    post_db_hash: B256,
    pre_evm_hash: B256,
    post_evm_hash: B256,
    results_hash: B256,
    block_ctx_hash: B256,
) -> B256 {
    let mut hasher = Sha256::new();
    hasher.update(tx_hash.as_slice());
    hasher.update(pre_db_hash.as_slice());
    hasher.update(post_db_hash.as_slice());
    hasher.update(pre_evm_hash.as_slice());
    hasher.update(post_evm_hash.as_slice());
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
    // AccountInfo: nonce, balance, code_hash. Skip `account_id` and `code` field
    // (see hash_results doc for the full exclusion rationale — code_hash binds
    // bytecode content via `validate_cached_contracts`).
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

/// Canonical SHA256 of a `Vec<ResultAndState<OpHaltReason>>`.
///
/// Committed into `chunk_trace` to authenticate the per-transaction execution trace.
/// Both the chunk guest (which computes this over results captured by a tracing EVM
/// wrapper during its own execution) and the aggregation guest (which computes it over
/// `Chunk.results` supplied by the witness) must arrive at the same hash — otherwise
/// the chunk's authenticated journal cannot be reconstructed and `env::verify()` fails.
///
/// # Upgrade hazard (review finding M-2, M-3): schema not versioned
///
/// This encoder is NOT versioned. The revm enum discriminants for `SuccessReason`,
/// `OutOfGasError`, `HaltReason`, and `OpHaltReason` are hardcoded (see
/// `halt_reason_byte`, `op_halt_reason_byte` etc. and the parallel mappings in
/// `rkyv/chunking.rs`). If a future revm release:
///   - adds a variant — Rust's exhaustive-match requirement will force a compile
///     error, which is safe.
///   - reorders variants for "documentation" purposes — the encoding silently shifts
///     byte values. Host and guest shift in lockstep (no cross-version mismatch within
///     a single build), but proofs from an older build become unverifiable.
///   - grows `Output`, `ExecutionResult`, `Log`, or `Account` with a new field — we
///     silently drop the new field from the hash. The aggregation still accepts the
///     chunk proof, but the binding is weaker by exactly that field.
///
/// Mitigation when upgrading revm: pin the revm version in Cargo.toml, add golden-vector
/// tests that pin specific `(variant, byte)` pairs (see `rkyv/chunking.rs` for the
/// parallel encoding that must stay in sync), and bump a schema-version byte folded
/// into `chunk_trace` when any field shape changes. None of these are implemented
/// here — this function is currently a stable-until-broken contract on revm's AST.
///
/// # Why transient fields are excluded
///
/// `Account.original_info`, `Account.transaction_id`, `EvmStorageSlot.transaction_id`,
/// `EvmStorageSlot.is_cold`, and `AccountInfo.code` (redundant with `code_hash`) are
/// all skipped. The first four are execution-context-dependent (a slot loaded during
/// tx 5 of a block has a different `transaction_id` than the same slot loaded in a
/// chunk that starts at tx 5), so including them would cause the chunk guest and
/// monolithic host to produce different hashes for functionally-identical traces.
/// `AccountInfo.code` is redundant: `code_hash` already commits to bytecode content,
/// so two `(code_hash, code_opt)` pairs with the same `code_hash` reduce to the same
/// logical account. (Review finding M-5: this means a witness with
/// `code_hash == KECCAK_EMPTY` and `code == Some(empty_bytes)` hashes the same as
/// `code == None`. That is a schema-drift channel, not a soundness bug — `code_hash`
/// still binds the authenticated bytecode via `validate_cached_contracts`.)
///
/// # Log topic count is not bounded here (M-4)
///
/// `write_logs` serializes `topics.len()` without asserting `len <= 4`. The EVM
/// LOG0..LOG4 opcodes enforce the 4-topic limit, so an honest chunk guest cannot
/// produce a log with more than 4 topics — any witness claiming such a log could
/// not correspond to any honestly-generated chunk proof, so `env::verify` would
/// reject it regardless. We rely on EVM semantics rather than a defensive assert
/// here; a future hardening pass could add the bound as a cheap extra guard.
pub fn hash_results(results: &[ResultAndState<OpHaltReason>]) -> B256 {
    let mut hasher = Sha256::new();
    write_u64(&mut hasher, results.len() as u64);
    for ras in results {
        write_execution_result(&mut hasher, &ras.result);
        write_evm_state(&mut hasher, &ras.state);
    }
    B256::from_slice(&hasher.finalize())
}

// ============================================================================
//  verify_block_chunks — aggregation-side binding of chunks to the block
// ============================================================================
//
// Verifies that a block's chunks (as supplied in the witness) are coherent with the
// block the aggregation pipeline actually derived. The chunk proof itself authenticates
// a `chunk_trace` containing `tx_hash`, pre/post DB + EVM hashes, and `results_hash`,
// but the *fields* `chunk.agreed_l2_output_root` and `chunk.parent_block_number` used
// by `stitch_chunks` to reconstruct the chunk's `ProofJournal` are witness-supplied —
// as is the assumption that `chunk.tx_hash` corresponds to the actual derivation-output
// txs. Without the checks below, an adversary could supply a valid chunk proof for
// some other block Y while aggregation is executing block X, since `ChunkingEvm`
// replays `chunk.results` verbatim regardless of what tx the block executor hands in.
//
// The three bindings together close the three forgery knobs identified by the
// adversarial review:
//   1. **Output-root anchor** — `chunk.agreed_l2_output_root` must equal the
//      aggregation pipeline's expected parent output root for this block (the previous
//      block's claimed output root, or `boot.agreed_l2_output_root` for the first
//      block). Because the L2 output root commits to the state root which commits to
//      all L2 state, equal output roots imply equal pre-state — this is the
//      cryptographic equivalent of asserting
//      `chunks[0].agreed_db == hash_cache_state(aggregation_pre_block_state)` without
//      the canonicalization mismatch between chunk-witness `Cache` (preloaded with
//      `NotExisting` stubs for all block-accessed addresses) and aggregation's
//      `State<TrieDB>.cache` (only prelude-touched addresses at the same boundary).
//   2. **Block number anchor** — `chunk.parent_block_number` must equal the derived
//      block's parent number. Combined with output-root equality, this pins the chunk
//      to this specific block in the derived chain.
//   3. **`tx_hash` binding** — `compute_tx_hash(txs[chunk_start..chunk_end])` over the
//      derivation-pipeline-supplied txs must equal `chunk.tx_hash`. This forces the
//      chunk proof to authenticate *exactly* the transactions the aggregation is
//      executing, closing the "T_Y ≠ T_X" substitution attack.
//
// Additionally this helper enforces:
//   - **Full coverage** — `sum(tx_count) == block_txs.len()`. Partial chunk coverage
//     is not currently supported (`ChunkingEvm` delegates to inner for uncovered txs,
//     which would run live — safe but out of model; reject to keep the model simple).
//     Tracks review finding M-9.
//   - **Hash-chain continuity** — `chunks[i+1].agreed_db == chunks[i].claimed_db` and
//     the same for `agreed_evm`/`claimed_evm`.
//   - **`tx_count == results.len()`** per chunk — malformed-witness guard.
//   - **`evm_state` self-consistency** — `hash_evm_state(&chunk.evm_state) ==
//     chunk.claimed_evm` for every chunk. Binds the structural `evm_state` field to
//     the authenticated `claimed_evm` hash so the final chunk's receipts can be
//     safely used for the receipts-root reconstruction check.
//   - **Receipts-root reconstruction** — `calculate_receipt_root(&last_chunk.evm_state
//     .receipts) == block_receipts_root`. Independent check that the authenticated
//     chunk receipts actually reproduce the block header's `receipts_root`, catching
//     any divergence between the chunk guest's captured receipts and the aggregation
//     block executor's produced receipts.
//   - **Block context binding (Codex round-3 critical)** — `chunk.block_env` and
//     `chunk.op_block_ctx` must match the derivation pipeline's actual block header
//     fields (timestamp, basefee, prevrandao, coinbase, number, gas_limit, difficulty,
//     parent_hash, parent_beacon_block_root, extra_data). Without this, an adversary
//     could generate a valid chunk proof under a forged execution context and have
//     aggregation accept those env-sensitive results (BASEFEE / PREVRANDAO / NUMBER /
//     COINBASE / TIMESTAMP / BLOBBASEFEE / BLOCKHASH / EIP-4788 beacon-root /
//     EIP-2935 ring / Holocene-Jovian EIP-1559 params in extra_data would all be
//     unchecked). The `block_ctx_hash` folded into `chunk_trace` by
//     `compute_chunk_trace` binds the chunk proof to whatever context it claims it
//     ran under; this header cross-check then forces that claimed context to equal
//     the actual derivation-produced context. Blob-excess-gas-and-price is
//     validated indirectly: it depends on the parent block's excess_blob_gas which
//     we do not have here, but the `pre_evm_hash` and `post_evm_hash` commitments
//     bind the blob accumulator state, and any mismatched blob pricing would
//     produce wrong blob tx gas accounting → mismatched `post_evm_hash` → chunk
//     proof rejection by the existing hash-chain and env continuity checks.

use alloy_consensus::proofs::calculate_receipt_root;
use alloy_consensus::{BlockHeader, Header};
use alloy_eips::eip7840::BlobParams;
use alloy_evm::op_revm::OpSpecId;
use alloy_evm::revm::context_interface::block::BlobExcessGasAndPrice;
use alloy_evm::revm::primitives::eip4844::{
    BLOB_BASE_FEE_UPDATE_FRACTION_CANCUN, BLOB_BASE_FEE_UPDATE_FRACTION_PRAGUE,
};
use anyhow::{bail, ensure};

use crate::executor::Chunk;

/// Iterate a block-indexed `chunks` vec and call [`verify_block_chunks`] for each
/// non-empty entry, deriving the `parent_header`, `spec_id`, and expected
/// `BlobExcessGasAndPrice` from derivation (or cached-execution) output.
///
/// Shared by both `run_core_client` CHUNK VERIFY phases (DERIVATION and
/// EXECUTION-ONLY branches) so the per-block cross-check logic lives in one place.
/// Without this helper the two callers duplicate ~25 lines of field-access and
/// context plumbing — a drift hazard any time the verification rules evolve.
///
/// ## Arguments
/// - `chunks_per_block` — outer index `i` corresponds to the block at
///   `safe_head_number + 1 + i`. Empty inner vecs are skipped.
/// - `executions` — derivation or cached-execution output, one entry per block in
///   the same order. `executions[block_idx]` is the `Execution` for block
///   `safe_head_number + 1 + block_idx`. Must be at least as long as the
///   longest non-empty chunks-for-block position (caller's responsibility; this
///   function returns a descriptive error if the bound is exceeded).
/// - `safe_head_parent` — the parent header for `executions[0]` (i.e., the
///   safe_head header itself). For later blocks the parent is taken from
///   `executions[block_idx - 1].artifacts.header`.
/// - `safe_head_number` — the block number of `safe_head_parent`.
/// - `rollup_config` — used to select the `spec_id` for each block's timestamp
///   (drives the hardfork-gated blob_gasprice update fraction).
pub fn verify_chunks_against_blocks(
    chunks_per_block: &[Vec<Chunk>],
    executions: &[&crate::executor::Execution],
    safe_head_parent: &Header,
    safe_head_number: u64,
    rollup_config: &kona_genesis::RollupConfig,
) -> anyhow::Result<()> {
    use anyhow::Context;
    for (block_idx, chunks_for_block) in chunks_per_block.iter().enumerate() {
        if chunks_for_block.is_empty() {
            continue;
        }
        let exec = executions.get(block_idx).copied().ok_or_else(|| {
            anyhow::anyhow!(
                "chunks supplied for block position {block_idx} but only {} blocks \
                 available for verification",
                executions.len()
            )
        })?;
        let expected_parent_block_number = safe_head_number + block_idx as u64;
        let expected_agreed_output_root = exec.agreed_output;
        let block_txs: Vec<Vec<u8>> = exec
            .attributes
            .transactions
            .as_ref()
            .map(|txs| txs.iter().map(|b| b.to_vec()).collect())
            .unwrap_or_default();
        let parent_header: &Header = if block_idx == 0 {
            safe_head_parent
        } else {
            executions[block_idx - 1].artifacts.header.inner()
        };
        let current_timestamp = exec.artifacts.header.inner().timestamp;
        let spec_id = rollup_config.spec_id(current_timestamp);
        let expected_blob = expected_blob_excess_gas_and_price(parent_header, spec_id);
        verify_block_chunks(
            chunks_for_block,
            expected_agreed_output_root,
            expected_parent_block_number,
            &block_txs,
            exec.artifacts.header.inner(),
            expected_blob,
        )
        .with_context(|| {
            format!(
                "chunk verification for block position {block_idx} \
                 (expected block {})",
                safe_head_number + 1 + block_idx as u64
            )
        })?;
    }
    Ok(())
}

/// Compute the `BlobExcessGasAndPrice` that kona's `prepare_block_env` would assign
/// to `BlockEnv.blob_excess_gas_and_price` for a block with the given parent header
/// and spec_id. Mirrors `kona/proof/executor/src/builder/env.rs::prepare_block_env`
/// exactly — if that implementation changes in upstream, this must too.
///
/// Used by [`run_core_client`](crate::client::core::run_core_client) to derive the
/// expected value to compare against `chunk.block_env.blob_excess_gas_and_price` in
/// [`verify_block_chunks`]. Authenticating this field is critical: on OP L2 the
/// BLOBBASEFEE opcode reads the blob gasprice, so a mismatched value changes
/// execution semantics for any contract that reads it.
pub fn expected_blob_excess_gas_and_price(
    parent_header: &Header,
    spec_id: OpSpecId,
) -> Option<BlobExcessGasAndPrice> {
    let (params, fraction) = if spec_id.is_enabled_in(OpSpecId::ISTHMUS) {
        (Some(BlobParams::prague()), BLOB_BASE_FEE_UPDATE_FRACTION_PRAGUE)
    } else if spec_id.is_enabled_in(OpSpecId::ECOTONE) {
        (Some(BlobParams::cancun()), BLOB_BASE_FEE_UPDATE_FRACTION_CANCUN)
    } else {
        (None, 0)
    };

    parent_header
        .maybe_next_block_excess_blob_gas(params)
        .or_else(|| spec_id.is_enabled_in(OpSpecId::ECOTONE).then_some(0))
        .map(|excess| BlobExcessGasAndPrice::new(excess, fraction))
}

/// Verify that a single block's chunks are coherent with the aggregation's derived block.
///
/// See the module comment above for the specific bindings and their rationale. Empty
/// chunk vecs are valid (block runs fully live through the inner EVM) — callers should
/// skip this function for such blocks.
///
/// # Panics
///
/// This function returns `anyhow::Error` rather than panicking so callers can surface
/// the first failing chunk / block index; all checks are preconditions for soundness,
/// so any failure must halt the aggregation proof.
pub fn verify_block_chunks(
    chunks: &[Chunk],
    expected_agreed_output_root: B256,
    expected_parent_block_number: u64,
    block_txs: &[Vec<u8>],
    block_header: &Header,
    expected_blob_excess_gas_and_price: Option<BlobExcessGasAndPrice>,
) -> anyhow::Result<()> {
    if chunks.is_empty() {
        // Nothing to verify — block runs fully live through the inner EVM.
        return Ok(());
    }

    // Per-chunk structural checks.
    let mut cursor: usize = 0;
    for (i, chunk) in chunks.iter().enumerate() {
        // Anchor 1: chunk commits to the expected parent output root.
        ensure!(
            chunk.agreed_l2_output_root == expected_agreed_output_root,
            "chunks[{i}].agreed_l2_output_root {} != expected {} — chunk may be for a \
             different L2 block; this is the 'wrong-block substitution' attack vector",
            chunk.agreed_l2_output_root,
            expected_agreed_output_root
        );

        // Anchor 2: chunk commits to the expected parent block number.
        ensure!(
            chunk.parent_block_number == expected_parent_block_number,
            "chunks[{i}].parent_block_number {} != expected {}",
            chunk.parent_block_number,
            expected_parent_block_number
        );

        // Block context binding (Codex round-3 critical).
        //
        // Verify each chunk's carried `block_env` / `op_block_ctx` matches the
        // derivation pipeline's actual header for this block. Combined with the
        // `block_ctx_hash` folded into `chunk_trace` by `compute_chunk_trace`, this
        // forces the chunk proof to have been generated under the exact execution
        // context the aggregation is using — any forged context would either fail
        // this cross-check (if chunk.block_env differs from header) or fail
        // `env::verify` in stitch_chunks (if chunk.block_env was tampered to match
        // this check but differs from what the chunk proof authenticated).
        let header = block_header;
        // BlockEnv.number — U256 form of the block number.
        ensure!(
            chunk.block_env.number == U256::from(header.number),
            "chunks[{i}].block_env.number {} != header.number {}",
            chunk.block_env.number,
            header.number
        );
        // BlockEnv.beneficiary — block coinbase.
        ensure!(
            chunk.block_env.beneficiary == header.beneficiary,
            "chunks[{i}].block_env.beneficiary {} != header.beneficiary {}",
            chunk.block_env.beneficiary,
            header.beneficiary
        );
        // BlockEnv.timestamp — U256 form of the block timestamp.
        ensure!(
            chunk.block_env.timestamp == U256::from(header.timestamp),
            "chunks[{i}].block_env.timestamp {} != header.timestamp {}",
            chunk.block_env.timestamp,
            header.timestamp
        );
        // BlockEnv.gas_limit — block gas limit.
        ensure!(
            chunk.block_env.gas_limit == header.gas_limit,
            "chunks[{i}].block_env.gas_limit {} != header.gas_limit {}",
            chunk.block_env.gas_limit,
            header.gas_limit
        );
        // BlockEnv.basefee — post-London, header carries it as Some(u64).
        ensure!(
            chunk.block_env.basefee == header.base_fee_per_gas.unwrap_or(0),
            "chunks[{i}].block_env.basefee {} != header.base_fee_per_gas {:?}",
            chunk.block_env.basefee,
            header.base_fee_per_gas
        );
        // BlockEnv.difficulty — typically 0 on OP L2 post-Merge but not guaranteed.
        ensure!(
            chunk.block_env.difficulty == header.difficulty,
            "chunks[{i}].block_env.difficulty {} != header.difficulty {}",
            chunk.block_env.difficulty,
            header.difficulty
        );
        // BlockEnv.prevrandao — post-Paris stored in header.mix_hash.
        ensure!(
            chunk.block_env.prevrandao == Some(header.mix_hash),
            "chunks[{i}].block_env.prevrandao {:?} != Some(header.mix_hash) {:?}",
            chunk.block_env.prevrandao,
            Some(header.mix_hash)
        );
        // BlockEnv.blob_excess_gas_and_price — must match what kona's block
        // builder would compute from the parent header + hardfork-selected
        // update fraction (see `prepare_block_env` in
        // `kona/proof/executor/src/builder/env.rs`). This closes the Codex
        // round-4 critical finding: the aggregation path replays witness-
        // supplied `chunk.results`, so pre/post EVM hash continuity alone is
        // only internal consistency between attacker-controlled chunks, not
        // binding to the real block context. On any block where blob pricing
        // affects execution (BLOBBASEFEE opcode, blob tx gas accounting) a
        // prover could otherwise generate a valid chunk proof under a forged
        // blob-pricing context and have aggregation accept the wrong post-
        // state/output root. We now reject on any mismatch.
        ensure!(
            chunk.block_env.blob_excess_gas_and_price == expected_blob_excess_gas_and_price,
            "chunks[{i}].block_env.blob_excess_gas_and_price {:?} != expected {:?}",
            chunk.block_env.blob_excess_gas_and_price,
            expected_blob_excess_gas_and_price
        );

        // OpBlockExecutionCtx.parent_hash — header.parent_hash.
        ensure!(
            chunk.op_block_ctx.parent_hash == header.parent_hash,
            "chunks[{i}].op_block_ctx.parent_hash {} != header.parent_hash {}",
            chunk.op_block_ctx.parent_hash,
            header.parent_hash
        );
        // OpBlockExecutionCtx.parent_beacon_block_root — header.parent_beacon_block_root.
        ensure!(
            chunk.op_block_ctx.parent_beacon_block_root == header.parent_beacon_block_root,
            "chunks[{i}].op_block_ctx.parent_beacon_block_root {:?} != header {:?}",
            chunk.op_block_ctx.parent_beacon_block_root,
            header.parent_beacon_block_root
        );
        // OpBlockExecutionCtx.extra_data — header.extra_data.
        ensure!(
            chunk.op_block_ctx.extra_data == header.extra_data,
            "chunks[{i}].op_block_ctx.extra_data {:?} != header.extra_data {:?}",
            chunk.op_block_ctx.extra_data,
            header.extra_data
        );

        // Malformed-witness guard.
        ensure!(
            chunk.tx_count as usize == chunk.results.len(),
            "chunks[{i}] tx_count ({}) != results.len() ({}) — malformed witness",
            chunk.tx_count,
            chunk.results.len()
        );

        // evm_state self-consistency: the structural field must hash to the
        // authenticated claimed_evm commitment. Without this, the receipts inside
        // evm_state are not bound to the chunk proof and cannot be safely used for the
        // receipts-root reconstruction below.
        ensure!(
            hash_evm_state(&chunk.evm_state) == chunk.claimed_evm,
            "chunks[{i}] evm_state hashes to {} but claimed_evm is {} — malformed or \
             tampered witness",
            hash_evm_state(&chunk.evm_state),
            chunk.claimed_evm
        );

        // tx_hash binding: the chunk's committed tx_hash must match the hash of the
        // derivation-pipeline-supplied txs for this chunk's range. Forces the chunk
        // proof to authenticate exactly the transactions the aggregation is executing.
        let chunk_end = cursor
            .checked_add(chunk.tx_count as usize)
            .ok_or_else(|| anyhow::anyhow!("chunks[{i}] tx_count overflow"))?;
        ensure!(
            chunk_end <= block_txs.len(),
            "chunks[{i}] extends past block tx count: end={chunk_end}, total={}",
            block_txs.len()
        );
        let expected_tx_hash = compute_tx_hash(&block_txs[cursor..chunk_end]);
        ensure!(
            chunk.tx_hash == expected_tx_hash,
            "chunks[{i}] tx_hash {} != expected {} over txs[{cursor}..{chunk_end}]",
            chunk.tx_hash,
            expected_tx_hash
        );
        cursor = chunk_end;
    }

    // Full-coverage: chunks must cover every tx in the block. (M-9: partial coverage
    // would let the tail run live through ChunkingEvm's inner delegation — safe for
    // state root but outside the Decision 12 model.)
    ensure!(
        cursor == block_txs.len(),
        "chunks cover {cursor} of {} block transactions — full coverage required",
        block_txs.len()
    );

    // Hash-chain continuity inside the block.
    for i in 1..chunks.len() {
        ensure!(
            chunks[i].agreed_db == chunks[i - 1].claimed_db,
            "chunk hash chain broken: agreed_db[{i}] != claimed_db[{j}]",
            j = i - 1
        );
        ensure!(
            chunks[i].agreed_evm == chunks[i - 1].claimed_evm,
            "chunk hash chain broken: agreed_evm[{i}] != claimed_evm[{j}]",
            j = i - 1
        );
    }

    // Receipts-root reconstruction: the last chunk's cumulative evm_state.receipts
    // must reproduce the block header's receipts_root. This authenticates that the
    // per-tx receipts captured by the chunk guest equal what the aggregation's block
    // executor produced — if the chunk replay diverged from honest execution (e.g., a
    // Frankenstein receipt built from a mismatched tx+result pair), the receipts_root
    // would differ.
    //
    // Safety: hash_evm_state check above ensures last_chunk.evm_state.receipts is
    // authenticated (via claimed_evm), so this root is computed over authenticated
    // input. `calculate_receipt_root` uses RLP EIP-2718 encoding, which
    // `OpReceiptEnvelope` implements via `Encodable2718`, matching the seal_block
    // computation upstream.
    let last = chunks.last().unwrap();
    let reconstructed = calculate_receipt_root(&last.evm_state.receipts);
    if reconstructed != block_header.receipts_root {
        bail!(
            "reconstructed receipts_root {} from last chunk's evm_state != block \
             receipts_root {}",
            reconstructed,
            block_header.receipts_root
        );
    }

    Ok(())
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
            account_id: None,
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
        let f = B256::repeat_byte(0x06);
        let g = B256::repeat_byte(0x07);

        let t1 = compute_chunk_trace(a, b, c, d, e, f, g);
        let t2 = compute_chunk_trace(a, b, c, d, e, f, g);
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
            B256::repeat_byte(0x06),
            B256::repeat_byte(0x07),
        ];
        let baseline = compute_chunk_trace(
            base[0], base[1], base[2], base[3], base[4], base[5], base[6],
        );

        for i in 0..7 {
            let mut modified = base;
            modified[i] = B256::repeat_byte(0xFF);
            let h = compute_chunk_trace(
                modified[0],
                modified[1],
                modified[2],
                modified[3],
                modified[4],
                modified[5],
                modified[6],
            );
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
        let h1 = hash_results(&[]);
        let h2 = hash_results(&[]);
        assert_eq!(h1, h2);
        assert!(!h1.is_zero(), "hash of empty trace is u64_be(0) hash");
    }

    #[test]
    fn hash_results_single_entry_deterministic() {
        let entry = stub_success(21000);
        let h1 = hash_results(std::slice::from_ref(&entry));
        let h2 = hash_results(std::slice::from_ref(&entry));
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_results_order_sensitive() {
        let a = stub_success(21000);
        let b = stub_success(42000);
        let h_ab = hash_results(&[a.clone(), b.clone()]);
        let h_ba = hash_results(&[b, a]);
        assert_ne!(
            h_ab, h_ba,
            "reordering results must change the hash (prevents aggregator-side reordering)"
        );
    }

    #[test]
    fn hash_results_gas_change_different() {
        let h1 = hash_results(&[stub_success(21000)]);
        let h2 = hash_results(&[stub_success(21001)]);
        assert_ne!(h1, h2);
    }

    #[test]
    fn hash_results_variant_change_different() {
        let success = hash_results(&[stub_success(21000)]);
        let revert = hash_results(&[stub_revert(21000, &[])]);
        assert_ne!(success, revert);
    }

    #[test]
    fn hash_results_state_change_different() {
        use alloy_evm::revm::state::EvmStorageSlot;
        let addr = Address::from([0xAA; 20]);

        // Baseline: empty state.
        let base = stub_success(21000);
        let h_base = hash_results(std::slice::from_ref(&base));

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
        let h_modified = hash_results(std::slice::from_ref(&modified));

        assert_ne!(
            h_base, h_modified,
            "state diff must contribute to results hash"
        );

        // Mutate a storage slot — hash must change.
        modified
            .state
            .get_mut(&addr)
            .unwrap()
            .storage
            .insert(U256::from(1), EvmStorageSlot::new_changed(U256::ZERO, U256::from(42), 0));
        let h_with_storage = hash_results(std::slice::from_ref(&modified));
        assert_ne!(h_modified, h_with_storage);
    }

    /// Transient fields (`transaction_id`, `is_cold`, `original_info`) must not
    /// contribute to the hash, so two logically-equivalent `ResultAndState` entries
    /// produced in different execution contexts yield the same hash.
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

        let mut ras1 = stub_success(21000);
        ras1.state.insert(
            addr,
            Account {
                info: info.clone(),
                original_info: Box::new(info.clone()),
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
                // Different original_info — must not affect hash.
                original_info: Box::new(AccountInfo::default()),
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
            hash_results(std::slice::from_ref(&ras1)),
            hash_results(std::slice::from_ref(&ras2)),
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

    // ========== compute_tx_hash tests (7.0) ==========

    #[test]
    fn tx_hash_deterministic() {
        let txs = vec![vec![0x01, 0x02], vec![0x03]];
        let h1 = compute_tx_hash(&txs);
        let h2 = compute_tx_hash(&txs);
        assert_eq!(h1, h2);
        assert!(!h1.is_zero());
    }

    #[test]
    fn tx_hash_ordering_sensitivity() {
        let txs_ab = vec![vec![0x01], vec![0x02]];
        let txs_ba = vec![vec![0x02], vec![0x01]];
        assert_ne!(compute_tx_hash(&txs_ab), compute_tx_hash(&txs_ba));
    }

    #[test]
    fn tx_hash_content_sensitivity() {
        let txs1 = vec![vec![0x01, 0x02]];
        let txs2 = vec![vec![0x01, 0x03]];
        assert_ne!(compute_tx_hash(&txs1), compute_tx_hash(&txs2));
    }

    #[test]
    fn tx_hash_empty_list() {
        let h = compute_tx_hash(&[]);
        assert!(!h.is_zero());
        // Empty list still produces a valid hash (just hashing the zero-length prefix)
        assert_eq!(h, compute_tx_hash(&[]));
    }

    #[test]
    fn tx_hash_length_prefix_prevents_ambiguity() {
        // [0x01, 0x02] as one tx vs [0x01], [0x02] as two txs must differ
        let single = vec![vec![0x01, 0x02]];
        let double = vec![vec![0x01], vec![0x02]];
        assert_ne!(compute_tx_hash(&single), compute_tx_hash(&double));
    }

    #[test]
    fn chunk_trace_integration_with_precondition() {
        let trace = compute_chunk_trace(
            B256::repeat_byte(0x01),
            B256::repeat_byte(0x02),
            B256::repeat_byte(0x03),
            B256::repeat_byte(0x04),
            B256::repeat_byte(0x05),
            B256::repeat_byte(0x06),
            B256::repeat_byte(0x07),
        );
        let p = crate::precondition::Precondition::default().chunk(trace);
        assert_eq!(p.digest(), risc0_zkvm::Digest::from_bytes(trace.0));
    }

    // ========== verify_block_chunks tests (items 1-3 adversarial review fix) ==========

    use crate::executor::Chunk;
    use op_alloy_consensus::{OpReceiptEnvelope, OpTxType};

    /// Build a Header whose fields are consistent with revm's `BlockEnv::default()`
    /// plus an unmodified `OpBlockExecutionCtx::default()`, so the block-context
    /// cross-check in `verify_block_chunks` passes when tests use `make_honest_chunk`
    /// with default context. Must mirror revm's Default impl — in particular
    /// `timestamp = 1`, `gas_limit = u64::MAX`, `mix_hash = B256::ZERO` (matches
    /// `prevrandao = Some(B256::ZERO)`).
    fn make_consistent_header(receipts_root: B256) -> Header {
        Header {
            receipts_root,
            timestamp: 1,
            gas_limit: u64::MAX,
            mix_hash: B256::ZERO,
            // All other fields default: number=0, beneficiary=zero, base_fee_per_gas=None,
            // difficulty=0, parent_hash=zero, parent_beacon_block_root=None,
            // extra_data=empty.
            ..Default::default()
        }
    }

    /// Build a well-formed chunk for a block with a single tx. All internal hashes are
    /// computed so any individual field mutation breaks the appropriate check in
    /// `verify_block_chunks` — tests then mutate one field at a time to pinpoint rejection.
    /// `block_env` / `op_block_ctx` are set so they match `make_consistent_header`'s
    /// default (and the block-context cross-check passes).
    fn make_honest_chunk(
        tx_bytes: Vec<u8>,
        agreed_output: B256,
        parent_block: u64,
        cumulative_gas: u64,
    ) -> (Chunk, OpReceiptEnvelope) {
        let receipt = OpReceiptEnvelope::from_parts(
            true,
            cumulative_gas,
            vec![],
            OpTxType::Legacy,
            None,
            None,
        );
        let evm_state = EvmAccumulatorState {
            cumulative_gas_used: cumulative_gas,
            da_footprint_used: 0,
            blob_gas_used: 0,
            logs_bloom: Bloom::ZERO,
            receipts: vec![receipt.clone()],
        };
        let claimed_evm = hash_evm_state(&evm_state);
        let tx_hash = compute_tx_hash(std::slice::from_ref(&tx_bytes));
        // BlockEnv::default() has: number=0, beneficiary=zero, timestamp=1,
        // gas_limit=u64::MAX, basefee=0, difficulty=0, prevrandao=Some(B256::ZERO).
        // Explicitly zero `blob_excess_gas_and_price` to None so the tests'
        // default `expected_blob_excess_gas_and_price = None` matches the
        // chunk's carried value — if upstream changes BlockEnv's default to
        // carry Some(_), we override here.
        let block_env = BlockEnv {
            blob_excess_gas_and_price: None,
            ..BlockEnv::default()
        };
        let chunk = Chunk {
            agreed_db: B256::repeat_byte(0x11),
            agreed_evm: B256::repeat_byte(0x22),
            tx_count: 1,
            tx_hash,
            results: vec![make_success_result_empty()],
            evm_state,
            claimed_db: B256::repeat_byte(0x33),
            claimed_evm,
            agreed_l2_output_root: agreed_output,
            parent_block_number: parent_block,
            block_env,
            op_block_ctx: OpBlockExecutionCtx::default(),
        };
        (chunk, receipt)
    }

    fn make_success_result_empty() -> ResultAndState<OpHaltReason> {
        ResultAndState {
            result: ExecutionResult::Success {
                reason: SuccessReason::Return,
                gas_used: 21000,
                gas_refunded: 0,
                logs: vec![],
                output: Output::Call(alloy_primitives::Bytes::new()),
            },
            state: Default::default(),
        }
    }

    /// Happy-path: a single honest chunk for a single-tx block passes all six checks.
    #[test]
    fn verify_block_chunks_happy_path() {
        let tx = vec![0xAA, 0xBB];
        let agreed = B256::repeat_byte(0x77);
        let parent = 1000u64;
        let (chunk, receipt) = make_honest_chunk(tx.clone(), agreed, parent, 21000);
        let receipts_root = calculate_receipt_root(&[receipt]);

        let header = make_consistent_header(receipts_root);
        verify_block_chunks(&[chunk], agreed, parent, &[tx], &header, None).unwrap();
    }

    /// Empty chunks vec is a no-op and must not error.
    #[test]
    fn verify_block_chunks_empty_is_ok() {
        verify_block_chunks(&[], B256::ZERO, 0, &[], &Header::default(), None).unwrap();
    }

    /// Output-root mismatch: adversary substitutes Y's chunk (different
    /// `agreed_l2_output_root`) into X's slot. Must be rejected with a clear message
    /// pointing at the wrong-block-substitution attack.
    #[test]
    fn verify_block_chunks_rejects_output_root_mismatch() {
        let tx = vec![0xAA];
        let agreed = B256::repeat_byte(0x77);
        let (chunk, receipt) = make_honest_chunk(tx.clone(), agreed, 10, 21000);
        let receipts_root = calculate_receipt_root(&[receipt]);

        // Aggregation expects a DIFFERENT output root for this block.
        let wrong_expected = B256::repeat_byte(0x88);
        let header = make_consistent_header(receipts_root);
        let err = verify_block_chunks(&[chunk], wrong_expected, 10, &[tx], &header, None)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("agreed_l2_output_root") && err.contains("wrong-block substitution"),
            "unexpected error: {err}"
        );
    }

    /// Parent block number mismatch: chunk claims to be for block N-1 but aggregation
    /// is at block M-1.
    #[test]
    fn verify_block_chunks_rejects_parent_block_mismatch() {
        let tx = vec![0xAA];
        let agreed = B256::repeat_byte(0x77);
        let (chunk, receipt) = make_honest_chunk(tx.clone(), agreed, 10, 21000);
        let receipts_root = calculate_receipt_root(&[receipt]);

        let header = make_consistent_header(receipts_root);
        let err = verify_block_chunks(&[chunk], agreed, 11, &[tx], &header, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("parent_block_number"), "unexpected error: {err}");
    }

    /// tx_count ≠ results.len() malformed-witness guard.
    #[test]
    fn verify_block_chunks_rejects_tx_count_results_mismatch() {
        let tx = vec![0xAA];
        let agreed = B256::repeat_byte(0x77);
        let (mut chunk, receipt) = make_honest_chunk(tx.clone(), agreed, 10, 21000);
        chunk.tx_count = 2; // Inconsistent with results.len() == 1.
        let receipts_root = calculate_receipt_root(&[receipt]);

        let header = make_consistent_header(receipts_root);
        let err = verify_block_chunks(&[chunk], agreed, 10, &[tx], &header, None)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("tx_count") && err.contains("results.len()"),
            "unexpected error: {err}"
        );
    }

    /// tx_hash binding: chunk committed a tx_hash for a different tx sequence. The
    /// bytes fed by derivation compute a different hash, so rejection.
    #[test]
    fn verify_block_chunks_rejects_tx_hash_mismatch() {
        let derivation_tx = vec![0xAA];
        let agreed = B256::repeat_byte(0x77);
        // Chunk has committed tx_hash for a DIFFERENT tx.
        let (mut chunk, receipt) = make_honest_chunk(vec![0xBB], agreed, 10, 21000);
        chunk.tx_hash = compute_tx_hash(&[vec![0xBB]]);
        let receipts_root = calculate_receipt_root(&[receipt]);

        // Now run verification with derivation's tx (0xAA), which hashes differently.
        let header = make_consistent_header(receipts_root);
        let err = verify_block_chunks(&[chunk], agreed, 10, &[derivation_tx], &header, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("tx_hash"), "unexpected error: {err}");
    }

    /// Full-coverage: if the sum of chunks' tx_counts is less than the block's tx
    /// count, aggregation would fall through to live execution on the tail — reject
    /// (M-9).
    #[test]
    fn verify_block_chunks_rejects_partial_coverage() {
        let tx1 = vec![0xAA];
        let tx2 = vec![0xBB];
        let agreed = B256::repeat_byte(0x77);
        let (chunk, receipt) = make_honest_chunk(tx1.clone(), agreed, 10, 21000);
        let receipts_root = calculate_receipt_root(&[receipt]);

        // Block has 2 txs but chunks only cover 1.
        let header = make_consistent_header(receipts_root);
        let err = verify_block_chunks(&[chunk], agreed, 10, &[tx1, tx2], &header, None)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("full coverage required"),
            "unexpected error: {err}"
        );
    }

    /// evm_state self-consistency: `hash_evm_state(&chunk.evm_state)` must equal
    /// `chunk.claimed_evm`. A mutated receipt in evm_state would invalidate the hash,
    /// so rejection.
    #[test]
    fn verify_block_chunks_rejects_evm_state_tampering() {
        let tx = vec![0xAA];
        let agreed = B256::repeat_byte(0x77);
        let (mut chunk, receipt) = make_honest_chunk(tx.clone(), agreed, 10, 21000);
        // Mutate evm_state (bump gas_used) without updating claimed_evm.
        chunk.evm_state.cumulative_gas_used += 1;
        let receipts_root = calculate_receipt_root(&[receipt]);

        let header = make_consistent_header(receipts_root);
        let err = verify_block_chunks(&[chunk], agreed, 10, &[tx], &header, None)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("evm_state") && err.contains("claimed_evm"),
            "unexpected error: {err}"
        );
    }

    /// Receipts-root reconstruction: the authenticated chunk receipts must reproduce
    /// the block's receipts_root. A block with a DIFFERENT header.receipts_root than
    /// what the chunks would produce is rejected.
    #[test]
    fn verify_block_chunks_rejects_receipts_root_divergence() {
        let tx = vec![0xAA];
        let agreed = B256::repeat_byte(0x77);
        let (chunk, _) = make_honest_chunk(tx.clone(), agreed, 10, 21000);

        // Pretend the block's header.receipts_root is something else entirely.
        let wrong_root = B256::repeat_byte(0xFF);
        let header = make_consistent_header(wrong_root);
        let err = verify_block_chunks(&[chunk], agreed, 10, &[tx], &header, None)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("receipts_root"),
            "unexpected error: {err}"
        );
    }

    /// Block-context binding (Codex round-3 critical): a chunk whose `block_env`
    /// disagrees with the derivation-produced header (e.g. forged timestamp) MUST
    /// be rejected. This is the check that closes the env-forgery attack: without
    /// it, an adversary could generate a valid chunk proof under a wrong-timestamp
    /// context and have aggregation accept its env-sensitive opcode results.
    #[test]
    fn verify_block_chunks_rejects_block_env_timestamp_mismatch() {
        let tx = vec![0xAA];
        let agreed = B256::repeat_byte(0x77);
        let (mut chunk, receipt) = make_honest_chunk(tx.clone(), agreed, 10, 21000);
        // Adversary: chunk was proven under timestamp=5 but derivation says 0.
        chunk.block_env.timestamp = U256::from(5);
        let receipts_root = calculate_receipt_root(&[receipt]);

        let header = make_consistent_header(receipts_root); // header.timestamp = 0
        let err = verify_block_chunks(&[chunk], agreed, 10, &[tx], &header, None)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("block_env.timestamp"),
            "unexpected error: {err}"
        );
    }

    /// Block-context binding: a chunk whose `block_env.basefee` disagrees with the
    /// header's `base_fee_per_gas` MUST be rejected — the adversarial review's
    /// "malicious prover under forged context" attack vector for BASEFEE opcode.
    #[test]
    fn verify_block_chunks_rejects_block_env_basefee_mismatch() {
        let tx = vec![0xAA];
        let agreed = B256::repeat_byte(0x77);
        let (mut chunk, receipt) = make_honest_chunk(tx.clone(), agreed, 10, 21000);
        chunk.block_env.basefee = 777;
        let receipts_root = calculate_receipt_root(&[receipt]);

        let header = make_consistent_header(receipts_root); // header.base_fee_per_gas = None
        let err = verify_block_chunks(&[chunk], agreed, 10, &[tx], &header, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("block_env.basefee"), "unexpected error: {err}");
    }

    /// Block-context binding: a chunk whose `block_env.beneficiary` (coinbase)
    /// disagrees with the header's beneficiary MUST be rejected — closes the
    /// COINBASE-opcode forgery path.
    #[test]
    fn verify_block_chunks_rejects_block_env_coinbase_mismatch() {
        let tx = vec![0xAA];
        let agreed = B256::repeat_byte(0x77);
        let (mut chunk, receipt) = make_honest_chunk(tx.clone(), agreed, 10, 21000);
        chunk.block_env.beneficiary = Address::from([0xAB; 20]);
        let receipts_root = calculate_receipt_root(&[receipt]);

        let header = make_consistent_header(receipts_root); // header.beneficiary = zero
        let err = verify_block_chunks(&[chunk], agreed, 10, &[tx], &header, None)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("block_env.beneficiary"),
            "unexpected error: {err}"
        );
    }

    /// Block-context binding: a chunk whose `op_block_ctx.parent_hash` disagrees
    /// with the header's parent_hash MUST be rejected — closes the EIP-2935
    /// parent-hash-ring / BLOCKHASH forgery path.
    #[test]
    fn verify_block_chunks_rejects_op_block_ctx_parent_hash_mismatch() {
        let tx = vec![0xAA];
        let agreed = B256::repeat_byte(0x77);
        let (mut chunk, receipt) = make_honest_chunk(tx.clone(), agreed, 10, 21000);
        chunk.op_block_ctx.parent_hash = B256::repeat_byte(0xDE);
        let receipts_root = calculate_receipt_root(&[receipt]);

        let header = make_consistent_header(receipts_root); // header.parent_hash = zero
        let err = verify_block_chunks(&[chunk], agreed, 10, &[tx], &header, None)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("op_block_ctx.parent_hash"),
            "unexpected error: {err}"
        );
    }

    /// Block-context binding: a chunk whose `op_block_ctx.extra_data` disagrees
    /// with the header's extra_data MUST be rejected — closes the Holocene/Jovian
    /// EIP-1559 params forgery path.
    #[test]
    fn verify_block_chunks_rejects_op_block_ctx_extra_data_mismatch() {
        let tx = vec![0xAA];
        let agreed = B256::repeat_byte(0x77);
        let (mut chunk, receipt) = make_honest_chunk(tx.clone(), agreed, 10, 21000);
        chunk.op_block_ctx.extra_data = alloy_primitives::Bytes::from_static(&[1, 2, 3]);
        let receipts_root = calculate_receipt_root(&[receipt]);

        let header = make_consistent_header(receipts_root); // header.extra_data = empty
        let err = verify_block_chunks(&[chunk], agreed, 10, &[tx], &header, None)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("op_block_ctx.extra_data"),
            "unexpected error: {err}"
        );
    }

    /// Hash-chain continuity: chunks[1].agreed_db must equal chunks[0].claimed_db.
    /// Broken chains are rejected.
    /// Codex round-4 critical regression test: a chunk whose
    /// `block_env.blob_excess_gas_and_price` differs from the expected (derivation-
    /// supplied) value must be rejected. This is the linchpin check that prevents a
    /// prover from authenticating a chunk proof under a forged blob-pricing context
    /// and then replaying those results during aggregation — see
    /// `verify_block_chunks`'s blob pricing block for the full rationale.
    #[test]
    fn verify_block_chunks_rejects_blob_price_forgery() {
        use alloy_evm::revm::context_interface::block::BlobExcessGasAndPrice;
        let agreed = B256::repeat_byte(0x77);
        let tx = vec![0xAA];
        let (chunk, receipt) = make_honest_chunk(tx.clone(), agreed, 10, 21000);
        let receipts_root = calculate_receipt_root(&[receipt]);
        let header = make_consistent_header(receipts_root);
        // Expected is None (no blob pricing supplied for this pre-Ecotone context),
        // but the chunk carries a forged value. Any non-None expected passed in
        // by the caller MUST match the chunk's value exactly.
        let forged_expected = Some(BlobExcessGasAndPrice::new(1_000_000, 3_338_477));
        let err =
            verify_block_chunks(&[chunk], agreed, 10, &[tx], &header, forged_expected)
                .unwrap_err()
                .to_string();
        assert!(
            err.contains("blob_excess_gas_and_price"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn verify_block_chunks_rejects_hash_chain_break_db() {
        let agreed = B256::repeat_byte(0x77);
        let (mut c0, r0) = make_honest_chunk(vec![0xAA], agreed, 10, 21000);
        let (mut c1, r1) = make_honest_chunk(vec![0xBB], agreed, 10, 42000);
        // Stitch chunks to cover both txs honestly, but break the db chain.
        c0.claimed_db = B256::repeat_byte(0xAA);
        c1.agreed_db = B256::repeat_byte(0xBB); // ≠ c0.claimed_db
        let receipts_root = calculate_receipt_root(&[r0, r1]);

        let header = make_consistent_header(receipts_root);
        let err = verify_block_chunks(&[c0, c1], agreed, 10, &[vec![0xAA], vec![0xBB]], &header, None)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("hash chain broken") && err.contains("agreed_db"),
            "unexpected error: {err}"
        );
    }
}

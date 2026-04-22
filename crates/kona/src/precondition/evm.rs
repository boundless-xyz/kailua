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

use alloy::signers::k256::sha2::digest::Update;
use alloy_evm::op_revm::OpHaltReason;
use alloy_evm::revm::context::BlockEnv;
use alloy_evm::revm::context_interface::result::{
    ExecutionResult, HaltReason, OutOfGasError, Output, ResultAndState, SuccessReason,
};
use alloy_evm::revm::database::in_memory_db::{AccountState, Cache, DbAccount};
use alloy_evm::revm::state::{Account, AccountInfo, AccountStatus, EvmState};
use alloy_op_evm::block::OpBlockExecutionCtx;
use alloy_primitives::{Address, B256, U256};
use risc0_zkvm::sha::rust_crypto::{Digest, Sha256};

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
pub fn hash_block_ctx(block_env: &BlockEnv, op_block_ctx: &OpBlockExecutionCtx) -> B256 {
    let mut hasher = Sha256::new();

    // --- BlockEnv ---
    hasher.update(block_env.number.to_be_bytes::<32>());
    hasher.update(block_env.beneficiary.0 .0);
    hasher.update(block_env.timestamp.to_be_bytes::<32>());
    write_u64(&mut hasher, block_env.gas_limit);
    write_u64(&mut hasher, block_env.basefee);
    hasher.update(block_env.difficulty.to_be_bytes::<32>());
    match &block_env.prevrandao {
        Some(r) => {
            hasher.update([1u8]);
            hasher.update(r.0);
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
    hasher.update(op_block_ctx.parent_hash.0);
    match &op_block_ctx.parent_beacon_block_root {
        Some(r) => {
            hasher.update([1u8]);
            hasher.update(r.0);
        }
        None => hasher.update([0u8]),
    }
    write_bytes(&mut hasher, &op_block_ctx.extra_data);

    B256::from_slice(&hasher.finalize())
}

fn write_u64(hasher: &mut Sha256, v: u64) {
    hasher.update(v.to_be_bytes());
}

fn write_bytes(hasher: &mut Sha256, data: &[u8]) {
    write_u64(hasher, data.len() as u64);
    hasher.update(data);
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
        hasher.update(log.address.0 .0);
        let topics = log.data.topics();
        write_u64(hasher, topics.len() as u64);
        for t in topics {
            hasher.update(t.0);
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
                    hasher.update(addr.0 .0);
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

fn write_account(hasher: &mut Sha256, acct: &Account) {
    // Pre-tx AccountInfo (original_info) — revm's first-load value for this address.
    // Authenticated so CachedEvm's serve-side `db.basic(addr)` check against
    // `account.original_info` is bound by the chunk proof.
    write_u64(hasher, acct.original_info.nonce);
    hasher.update(acct.original_info.balance.to_be_bytes::<32>());
    hasher.update(acct.original_info.code_hash.0);

    // Post-tx AccountInfo (info): nonce, balance, code_hash. Skip `account_id` and
    // `code` field (code_hash binds bytecode content via `validate_cached_contracts`).
    write_u64(hasher, acct.info.nonce);
    hasher.update(acct.info.balance.to_be_bytes::<32>());
    hasher.update(acct.info.code_hash.0);

    // Status bitflags (u8 — see M-6 assertion above).
    hasher.update([acct.status.bits()]);

    // Storage, sorted by slot key.
    let mut entries: Vec<_> = acct.storage.iter().collect();
    entries.sort_by_key(|(k, _)| *k);
    write_u64(hasher, entries.len() as u64);
    for (slot, evm_slot) in entries {
        hasher.update(slot.to_be_bytes::<32>());
        hasher.update(evm_slot.original_value.to_be_bytes::<32>());
        hasher.update(evm_slot.present_value.to_be_bytes::<32>());
    }
}

/// Encode an `EvmState` (the per-tx state diff). Entries are sorted by address so the
/// encoding is invariant to the underlying `HashMap` iteration order.
fn write_evm_state(hasher: &mut Sha256, state: &EvmState) {
    let mut entries: Vec<_> = state.iter().collect();
    entries.sort_by_key(|(addr, _)| *addr);
    write_u64(hasher, entries.len() as u64);
    for (addr, account) in entries {
        hasher.update(addr.0 .0);
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
        // cache must record it as NotExisting so later chunks' witness caches are
        // populated with the authentic pre-state view.
        use crate::evm::state::apply_trace_to_cache;
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

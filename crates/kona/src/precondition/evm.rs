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

use crate::evm::expected::{ExpectedAccount, ExpectedStateEntry};
use crate::evm::partial::{PartialAccount, PartialResultAndState};
use crate::precondition::derivation::flatten_bytes;
use alloy_evm::op_revm::OpHaltReason;
use alloy_evm::revm::context::BlockEnv;
use alloy_evm::revm::context_interface::block::BlobExcessGasAndPrice;
use alloy_evm::revm::context_interface::result::{
    ExecutionResult, HaltReason, OutOfGasError, Output, SuccessReason,
};
use alloy_op_evm::block::OpBlockExecutionCtx;
use alloy_primitives::{Address, Log, B256};
use risc0_zkvm::sha::{Impl as SHA2, Sha256};

/// Compute the chunk_trace commitment from the three input hashes.
///
/// Returns:
/// ```text
/// SHA256(results_hash || block_ctx_hash || expected_state_hash)
/// ```
pub fn compute_pe_trace(
    results_hash: B256,
    block_ctx_hash: B256,
    expected_state_hash: B256,
) -> B256 {
    let hashed_bytes = [
        results_hash.as_slice(),
        block_ctx_hash.as_slice(),
        expected_state_hash.as_slice(),
    ]
    .concat();
    let digest: [u8; 32] = SHA2::hash_bytes(hashed_bytes.as_slice())
        .as_bytes()
        .try_into()
        .unwrap();
    digest.into()
}

/// Canonical SHA256 of the block execution context (`BlockEnv` + `OpBlockExecutionCtx`).
pub fn hash_block_ctx(block_env: &BlockEnv, op_block_ctx: &OpBlockExecutionCtx) -> B256 {
    let hashed_bytes = [
        flatten_block_env(block_env).as_slice(),
        flatten_op_block_execution_ctx(op_block_ctx).as_slice(),
    ]
    .concat();
    let digest: [u8; 32] = SHA2::hash_bytes(hashed_bytes.as_slice())
        .as_bytes()
        .try_into()
        .unwrap();
    digest.into()
}

/// Canonical SHA256 of a list of txn hashes and their
/// `Vec<PartialResultAndState>`. State must be sorted by address (invariant
/// upheld by [`PartialResultAndState`]'s `From<ResultAndState>` impl);
/// per-account storage must likewise be sorted by slot. Both are walked in
/// iteration order without further sorting, so the byte output is stable for
/// any input that satisfies these invariants.
pub fn hash_results(tx_hashes: &[B256], results: &[PartialResultAndState]) -> B256 {
    assert_eq!(
        tx_hashes.len(),
        results.len(),
        "hash_results: tx_hashes and results must have the same length",
    );
    let hashed_bytes = [
        (results.len() as u64).to_be_bytes().as_slice(),
        tx_hashes
            .iter()
            .zip(results)
            .map(|(tx_hash, ras)| {
                [
                    tx_hash.as_slice(),
                    flatten_bytes(flatten_execution_result(&ras.result)).as_slice(),
                    flatten_bytes(flatten_partial_state(&ras.state)).as_slice(),
                ]
                .concat()
            })
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
    ]
    .concat();
    let digest: [u8; 32] = SHA2::hash_bytes(hashed_bytes.as_slice())
        .as_bytes()
        .try_into()
        .unwrap();
    digest.into()
}

/// Canonical SHA256 of the expected state carried by a partial execution.
pub fn hash_expected_state(expected_state: &[ExpectedStateEntry]) -> B256 {
    let digest: [u8; 32] = SHA2::hash_bytes(flatten_expected_state(expected_state).as_slice())
        .as_bytes()
        .try_into()
        .unwrap();
    digest.into()
}

pub fn flatten_block_env(block_env: &BlockEnv) -> Vec<u8> {
    [
        block_env.number.to_be_bytes::<32>().as_slice(),
        block_env.beneficiary.as_slice(),
        block_env.timestamp.to_be_bytes::<32>().as_slice(),
        block_env.gas_limit.to_be_bytes().as_slice(),
        block_env.basefee.to_be_bytes().as_slice(),
        block_env.difficulty.to_be_bytes::<32>().as_slice(),
        flatten_opt_prevrandao(block_env.prevrandao.as_ref()).as_slice(),
        flatten_opt_blob_excess(block_env.blob_excess_gas_and_price.as_ref()).as_slice(),
    ]
    .concat()
}

pub fn flatten_op_block_execution_ctx(ctx: &OpBlockExecutionCtx) -> Vec<u8> {
    [
        ctx.parent_hash.as_slice(),
        flatten_opt_prevrandao(ctx.parent_beacon_block_root.as_ref()).as_slice(),
        flatten_bytes(&ctx.extra_data).as_slice(),
    ]
    .concat()
}

fn flatten_opt_prevrandao(opt: Option<&B256>) -> Vec<u8> {
    match opt {
        Some(r) => [&[1u8], r.as_slice()].concat(),
        None => vec![0u8],
    }
}

fn flatten_opt_blob_excess(opt: Option<&BlobExcessGasAndPrice>) -> Vec<u8> {
    match opt {
        Some(b) => [
            [1u8].as_slice(),
            b.excess_blob_gas.to_be_bytes().as_slice(),
            b.blob_gasprice.to_be_bytes().as_slice(),
        ]
        .concat(),
        None => vec![0u8],
    }
}

/// Discriminants for `SuccessReason`. Matches the ordering in `rkyv/evm.rs`
/// (`success_reason_byte`) so both encodings stay in sync.
fn success_reason_disc(r: &SuccessReason) -> u8 {
    match r {
        SuccessReason::Stop => 0,
        SuccessReason::Return => 1,
        SuccessReason::SelfDestruct => 2,
    }
}

/// Discriminants for `OutOfGasError`. Matches `rkyv/evm.rs::oog_byte`.
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

/// Discriminants for `HaltReason`. Matches `rkyv/evm.rs::halt_reason_rkyv`.
fn flatten_halt_reason(r: &HaltReason) -> Vec<u8> {
    match r {
        HaltReason::OutOfGas(e) => vec![0, oog_disc(e)],
        HaltReason::OpcodeNotFound => vec![1, 0],
        HaltReason::InvalidFEOpcode => vec![2, 0],
        HaltReason::InvalidJump => vec![3, 0],
        HaltReason::NotActivated => vec![4, 0],
        HaltReason::StackUnderflow => vec![5, 0],
        HaltReason::StackOverflow => vec![6, 0],
        HaltReason::OutOfOffset => vec![7, 0],
        HaltReason::CreateCollision => vec![8, 0],
        HaltReason::PrecompileError => vec![9, 0],
        HaltReason::PrecompileErrorWithContext(s) => {
            [[10u8, 0].as_slice(), flatten_bytes(s.as_bytes()).as_slice()].concat()
        }
        HaltReason::NonceOverflow => vec![11, 0],
        HaltReason::CreateContractSizeLimit => vec![12, 0],
        HaltReason::CreateContractStartingWithEF => vec![13, 0],
        HaltReason::CreateInitCodeSizeLimit => vec![14, 0],
        HaltReason::OverflowPayment => vec![15, 0],
        HaltReason::StateChangeDuringStaticCall => vec![16, 0],
        HaltReason::CallNotAllowedInsideStatic => vec![17, 0],
        HaltReason::OutOfFunds => vec![18, 0],
        HaltReason::CallTooDeep => vec![19, 0],
    }
}

fn flatten_op_halt_reason(r: &OpHaltReason) -> Vec<u8> {
    match r {
        OpHaltReason::Base(h) => [[0u8].as_slice(), flatten_halt_reason(h).as_slice()].concat(),
        OpHaltReason::FailedDeposit => vec![1u8],
    }
}

fn flatten_log(log: &Log) -> Vec<u8> {
    let topics = log.data.topics();
    [
        log.address.as_slice(),
        (topics.len() as u64).to_be_bytes().as_slice(),
        topics
            .iter()
            .map(|t| t.0)
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
        flatten_bytes(&log.data.data).as_slice(),
    ]
    .concat()
}

fn flatten_logs(logs: &[Log]) -> Vec<u8> {
    [
        (logs.len() as u64).to_be_bytes().as_slice(),
        logs.iter()
            .map(flatten_log)
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
    ]
    .concat()
}

fn flatten_opt_address(opt: Option<&Address>) -> Vec<u8> {
    match opt {
        Some(addr) => [[1u8].as_slice(), addr.as_slice()].concat(),
        None => vec![0u8],
    }
}

fn flatten_output(output: &Output) -> Vec<u8> {
    match output {
        Output::Call(data) => [[0u8].as_slice(), flatten_bytes(data).as_slice()].concat(),
        Output::Create(data, addr_opt) => [
            [1u8].as_slice(),
            flatten_bytes(data).as_slice(),
            flatten_opt_address(addr_opt.as_ref()).as_slice(),
        ]
        .concat(),
    }
}

pub fn flatten_execution_result(r: &ExecutionResult<OpHaltReason>) -> Vec<u8> {
    match r {
        ExecutionResult::Success {
            reason,
            gas_used,
            gas_refunded,
            logs,
            output,
        } => [
            [0u8, success_reason_disc(reason)].as_slice(),
            gas_used.to_be_bytes().as_slice(),
            gas_refunded.to_be_bytes().as_slice(),
            flatten_logs(logs).as_slice(),
            flatten_output(output).as_slice(),
        ]
        .concat(),
        ExecutionResult::Revert { gas_used, output } => [
            [1u8].as_slice(),
            gas_used.to_be_bytes().as_slice(),
            flatten_bytes(output).as_slice(),
        ]
        .concat(),
        ExecutionResult::Halt { reason, gas_used } => [
            [2u8].as_slice(),
            flatten_op_halt_reason(reason).as_slice(),
            gas_used.to_be_bytes().as_slice(),
        ]
        .concat(),
    }
}

fn flatten_partial_account(acct: &PartialAccount) -> Vec<u8> {
    debug_assert!(
        acct.storage.windows(2).all(|w| w[0].slot <= w[1].slot),
        "flatten_partial_account: storage must be sorted by slot",
    );
    [
        // Pre-tx AccountInfo (original_info) — revm's first-load value for this address.
        // Authenticated so CachedEvm's serve-side `db.basic(addr)` check against
        // `account.original_info` is bound by the chunk proof.
        acct.original_info.nonce.to_be_bytes().as_slice(),
        acct.original_info.balance.to_be_bytes::<32>().as_slice(),
        acct.original_info.code_hash.as_slice(),
        // Post-tx AccountInfo (info): nonce, balance, code_hash. Skip `account_id` and
        // `code` field (code_hash binds bytecode content via `validate_cached_contracts`).
        acct.info.nonce.to_be_bytes().as_slice(),
        acct.info.balance.to_be_bytes::<32>().as_slice(),
        acct.info.code_hash.as_slice(),
        // Status bitflags (u8).
        [acct.status.bits()].as_slice(),
        // Storage, in the already-sorted Vec order maintained by `PartialAccount`.
        (acct.storage.len() as u64).to_be_bytes().as_slice(),
        acct.storage
            .iter()
            .map(|entry| {
                [
                    entry.slot.to_be_bytes::<32>().as_slice(),
                    entry
                        .slot_value
                        .original_value
                        .to_be_bytes::<32>()
                        .as_slice(),
                    entry
                        .slot_value
                        .present_value
                        .to_be_bytes::<32>()
                        .as_slice(),
                ]
                .concat()
            })
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
    ]
    .concat()
}

fn flatten_expected_account(acct: &ExpectedAccount) -> Vec<u8> {
    debug_assert!(
        acct.storage.windows(2).all(|w| w[0].slot <= w[1].slot),
        "flatten_expected_account: storage must be sorted by slot",
    );
    [
        [acct.exists as u8].as_slice(),
        acct.info.nonce.to_be_bytes().as_slice(),
        acct.info.balance.to_be_bytes::<32>().as_slice(),
        acct.info.code_hash.as_slice(),
        (acct.storage.len() as u64).to_be_bytes().as_slice(),
        acct.storage
            .iter()
            .map(|entry| {
                [
                    entry.slot.to_be_bytes::<32>().as_slice(),
                    entry.value.to_be_bytes::<32>().as_slice(),
                ]
                .concat()
            })
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
    ]
    .concat()
}

pub fn flatten_expected_state(state: &[ExpectedStateEntry]) -> Vec<u8> {
    debug_assert!(
        state.windows(2).all(|w| w[0].address <= w[1].address),
        "flatten_expected_state: state must be sorted by address",
    );
    [
        (state.len() as u64).to_be_bytes().as_slice(),
        state
            .iter()
            .map(|entry| {
                [
                    entry.address.as_slice(),
                    flatten_bytes(flatten_expected_account(&entry.account)).as_slice(),
                ]
                .concat()
            })
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
    ]
    .concat()
}

/// Encode the per-tx state diff carried by [`PartialResultAndState`]. The Vec
/// is sorted by address by construction, so we walk it directly.
pub fn flatten_partial_state(state: &[crate::evm::partial::PartialStateEntry]) -> Vec<u8> {
    debug_assert!(
        state.windows(2).all(|w| w[0].address <= w[1].address),
        "flatten_partial_state: state must be sorted by address",
    );
    [
        (state.len() as u64).to_be_bytes().as_slice(),
        state
            .iter()
            .map(|entry| {
                [
                    entry.address.as_slice(),
                    flatten_bytes(flatten_partial_account(&entry.account)).as_slice(),
                ]
                .concat()
            })
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
    ]
    .concat()
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use alloy_evm::revm::context_interface::result::ResultAndState;
    use alloy_evm::revm::primitives::HashMap;
    use alloy_evm::revm::state::{Account, AccountInfo, Bytecode};
    use alloy_primitives::U256;
    use risc0_zkvm::sha::Digestible;

    pub fn make_bytecode(bytes: &'static [u8]) -> Bytecode {
        Bytecode::new_raw(alloy_primitives::Bytes::from_static(bytes))
    }

    // ========== compute_chunk_trace tests ==========

    #[test]
    fn chunk_trace_deterministic() {
        let a = B256::repeat_byte(0x01);
        let b = B256::repeat_byte(0x02);
        let c = B256::repeat_byte(0x03);
        let t1 = compute_pe_trace(a, b, c);
        let t2 = compute_pe_trace(a, b, c);
        assert_eq!(t1, t2);
        assert!(!t1.is_zero());
    }

    #[test]
    fn chunk_trace_any_input_change_different() {
        let base = [
            B256::repeat_byte(0x01),
            B256::repeat_byte(0x02),
            B256::repeat_byte(0x03),
        ];
        let baseline = compute_pe_trace(base[0], base[1], base[2]);
        for i in 0..3 {
            let mut modified = base;
            modified[i] = B256::repeat_byte(0xFF);
            let h = compute_pe_trace(modified[0], modified[1], modified[2]);
            assert_ne!(
                baseline, h,
                "changing input {i} should produce different trace"
            );
        }
    }

    #[test]
    fn hash_expected_state_storage_value_change_different() {
        let make_state = |value| {
            vec![ExpectedStateEntry {
                address: Address::ZERO,
                account: ExpectedAccount {
                    exists: true,
                    info: AccountInfo::default(),
                    storage: vec![crate::evm::expected::ExpectedStorageEntry {
                        slot: U256::from(1),
                        value,
                    }],
                },
            }]
        };

        assert_ne!(
            hash_expected_state(&make_state(U256::ZERO)),
            hash_expected_state(&make_state(U256::from(1))),
        );
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
        let entry = PartialResultAndState::from(stub_success(21000));
        let h1 = hash_results(&[B256::ZERO], std::slice::from_ref(&entry));
        let h2 = hash_results(&[B256::ZERO], std::slice::from_ref(&entry));
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_results_order_sensitive() {
        let a = PartialResultAndState::from(stub_success(21000));
        let b = PartialResultAndState::from(stub_success(42000));
        let h_ab = hash_results(&[B256::ZERO, B256::ZERO], &[a.clone(), b.clone()]);
        let h_ba = hash_results(&[B256::ZERO, B256::ZERO], &[b, a]);
        assert_ne!(
            h_ab, h_ba,
            "reordering results must change the hash (prevents aggregator-side reordering)"
        );
    }

    #[test]
    fn hash_results_gas_change_different() {
        let h1 = hash_results(
            &[B256::ZERO],
            &[PartialResultAndState::from(stub_success(21000))],
        );
        let h2 = hash_results(
            &[B256::ZERO],
            &[PartialResultAndState::from(stub_success(21001))],
        );
        assert_ne!(h1, h2);
    }

    #[test]
    fn hash_results_variant_change_different() {
        let success = hash_results(
            &[B256::ZERO],
            &[PartialResultAndState::from(stub_success(21000))],
        );
        let revert = hash_results(
            &[B256::ZERO],
            &[PartialResultAndState::from(stub_revert(21000, &[]))],
        );
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
            std::slice::from_ref(&PartialResultAndState::from(make_ras(
                AccountInfo::default(),
            ))),
        );
        let h_nonzero = hash_results(
            &[B256::ZERO],
            std::slice::from_ref(&PartialResultAndState::from(make_ras(AccountInfo {
                nonce: 5,
                balance: U256::from(999),
                code_hash: B256::ZERO,
                account_id: None,
                code: None,
            }))),
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
        let base = PartialResultAndState::from(stub_success(21000));
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
        let h_modified = hash_results(
            &[B256::ZERO],
            std::slice::from_ref(&PartialResultAndState::from(modified.clone())),
        );

        assert_ne!(
            h_base, h_modified,
            "state diff must contribute to results hash"
        );

        // Mutate a storage slot — hash must change.
        modified.state.get_mut(&addr).unwrap().storage.insert(
            U256::from(1),
            EvmStorageSlot::new_changed(U256::ZERO, U256::from(42), 0),
        );
        let h_with_storage = hash_results(
            &[B256::ZERO],
            std::slice::from_ref(&PartialResultAndState::from(modified)),
        );
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
            hash_results(
                &[B256::ZERO],
                std::slice::from_ref(&PartialResultAndState::from(ras1))
            ),
            hash_results(
                &[B256::ZERO],
                std::slice::from_ref(&PartialResultAndState::from(ras2))
            ),
            "transient fields must not affect the canonical hash"
        );
    }

    #[test]
    fn chunk_trace_integration_with_precondition() {
        let trace = compute_pe_trace(
            B256::repeat_byte(0x01),
            B256::repeat_byte(0x02),
            B256::repeat_byte(0x03),
        );
        let p = crate::precondition::Precondition::default().partial(trace);
        assert_eq!(p.digest(), risc0_zkvm::Digest::from_bytes(trace.0));
    }
}

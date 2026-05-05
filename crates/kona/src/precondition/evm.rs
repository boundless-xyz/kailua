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
    use crate::evm::expected::ExpectedStorageEntry;
    use crate::evm::partial::{PartialAccount, PartialStateEntry, PartialStorageEntry};
    use alloy_evm::revm::context_interface::result::ResultAndState;
    use alloy_evm::revm::primitives::HashMap;
    use alloy_evm::revm::state::{Account, AccountInfo, AccountStatus, EvmStorageSlot};
    use alloy_primitives::{Bytes, U256};
    use risc0_zkvm::sha::Digestible;

    // ---------- Builder helpers (production-shape stubs) ----------

    fn stub_success(gas_used: u64) -> ResultAndState<OpHaltReason> {
        ResultAndState {
            result: ExecutionResult::Success {
                reason: SuccessReason::Return,
                gas_used,
                gas_refunded: 0,
                logs: vec![],
                output: Output::Call(Bytes::new()),
            },
            state: Default::default(),
        }
    }

    fn stub_revert(gas_used: u64, output: &[u8]) -> ResultAndState<OpHaltReason> {
        ResultAndState {
            result: ExecutionResult::Revert {
                gas_used,
                output: Bytes::copy_from_slice(output),
            },
            state: Default::default(),
        }
    }

    fn stub_halt(gas_used: u64, reason: OpHaltReason) -> ResultAndState<OpHaltReason> {
        ResultAndState {
            result: ExecutionResult::Halt { reason, gas_used },
            state: Default::default(),
        }
    }

    fn all_success_reasons() -> [SuccessReason; 3] {
        [
            SuccessReason::Stop,
            SuccessReason::Return,
            SuccessReason::SelfDestruct,
        ]
    }

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

    /// All 20 `HaltReason` variants. Mirrors the discriminant ordering in
    /// `flatten_halt_reason` — adding a variant upstream and not adding it here
    /// will silently drop coverage for that branch.
    fn all_halt_reasons() -> Vec<HaltReason> {
        vec![
            HaltReason::OutOfGas(OutOfGasError::Basic),
            HaltReason::OpcodeNotFound,
            HaltReason::InvalidFEOpcode,
            HaltReason::InvalidJump,
            HaltReason::NotActivated,
            HaltReason::StackUnderflow,
            HaltReason::StackOverflow,
            HaltReason::OutOfOffset,
            HaltReason::CreateCollision,
            HaltReason::PrecompileError,
            HaltReason::PrecompileErrorWithContext("ctx".to_string()),
            HaltReason::NonceOverflow,
            HaltReason::CreateContractSizeLimit,
            HaltReason::CreateContractStartingWithEF,
            HaltReason::CreateInitCodeSizeLimit,
            HaltReason::OverflowPayment,
            HaltReason::StateChangeDuringStaticCall,
            HaltReason::CallNotAllowedInsideStatic,
            HaltReason::OutOfFunds,
            HaltReason::CallTooDeep,
        ]
    }

    /// Build a revm `Account` with a sparse storage map, ready to be folded
    /// through `PartialResultAndState::from` into a sorted `PartialAccount`.
    fn make_account(
        info_nonce: u64,
        info_balance: U256,
        original_nonce: u64,
        status: AccountStatus,
        storage_entries: &[(U256, U256, U256)], // (slot, original, present)
    ) -> Account {
        let mut storage: HashMap<U256, EvmStorageSlot> = Default::default();
        for (slot, original, present) in storage_entries {
            storage.insert(*slot, EvmStorageSlot::new_changed(*original, *present, 0));
        }
        Account {
            info: AccountInfo {
                nonce: info_nonce,
                balance: info_balance,
                code_hash: B256::ZERO,
                account_id: None,
                code: None,
            },
            original_info: Box::new(AccountInfo {
                nonce: original_nonce,
                balance: U256::ZERO,
                code_hash: B256::ZERO,
                account_id: None,
                code: None,
            }),
            transaction_id: 0,
            storage,
            status,
        }
    }

    fn log_with(addr: Address, topics: Vec<B256>, data: &[u8]) -> Log {
        Log::new_unchecked(addr, topics, Bytes::copy_from_slice(data))
    }

    // ---------- compute_pe_trace ----------

    /// Determinism + per-input change in one body. The 3-input shape is small
    /// enough to walk every position exhaustively.
    #[test]
    fn compute_pe_trace_matrix() {
        let base = [
            B256::repeat_byte(0x01),
            B256::repeat_byte(0x02),
            B256::repeat_byte(0x03),
        ];
        let baseline = compute_pe_trace(base[0], base[1], base[2]);
        assert_eq!(
            baseline,
            compute_pe_trace(base[0], base[1], base[2]),
            "deterministic"
        );
        assert!(!baseline.is_zero());

        for i in 0..3 {
            let mut modified = base;
            modified[i] = B256::repeat_byte(0xFF);
            assert_ne!(
                baseline,
                compute_pe_trace(modified[0], modified[1], modified[2]),
                "changing input {i} must change trace",
            );
        }
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

    // ---------- hash_block_ctx ----------

    /// Walks every (prevrandao Some/None) × (blob_excess Some/None) ×
    /// (parent_beacon_block_root Some/None) × (extra_data empty/non-empty)
    /// combination, then perturbs each scalar/option field of `BlockEnv` and
    /// `OpBlockExecutionCtx` from a fully-populated baseline. Together this
    /// exercises every branch of `flatten_block_env`,
    /// `flatten_op_block_execution_ctx`, `flatten_opt_prevrandao`, and
    /// `flatten_opt_blob_excess`, and confirms that no field can be tampered
    /// with without invalidating `hash_block_ctx`.
    #[test]
    fn hash_block_ctx_dense() {
        let prev = B256::repeat_byte(0xCC);
        let pbbr = B256::repeat_byte(0xDD);
        let blob = BlobExcessGasAndPrice {
            excess_blob_gas: 1_000,
            blob_gasprice: u128::MAX, // pushes the [u8; 16] big-endian path
        };

        let make_be =
            |prevrandao: Option<B256>, blob_excess: Option<BlobExcessGasAndPrice>| -> BlockEnv {
                BlockEnv {
                    number: U256::from(123u64),
                    beneficiary: Address::from([0xAA; 20]),
                    timestamp: U256::from(1_000u64),
                    gas_limit: 30_000_000,
                    basefee: 7,
                    difficulty: U256::ZERO,
                    prevrandao,
                    blob_excess_gas_and_price: blob_excess,
                }
            };
        let make_ctx = |parent_beacon_block_root: Option<B256>, extra: &[u8]| OpBlockExecutionCtx {
            parent_hash: B256::repeat_byte(0xBB),
            parent_beacon_block_root,
            extra_data: Bytes::copy_from_slice(extra),
        };

        // 8 distinct configurations across all option combos × extra_data sizes.
        let cases: Vec<(BlockEnv, OpBlockExecutionCtx)> = vec![
            (make_be(None, None), make_ctx(None, &[])),
            (make_be(Some(prev), None), make_ctx(None, &[])),
            (make_be(None, Some(blob)), make_ctx(None, &[])),
            (make_be(Some(prev), Some(blob)), make_ctx(None, &[])),
            (make_be(None, None), make_ctx(Some(pbbr), &[])),
            (make_be(None, None), make_ctx(None, b"x")),
            (make_be(Some(prev), Some(blob)), make_ctx(Some(pbbr), b"hi")),
            (
                make_be(Some(prev), Some(blob)),
                make_ctx(Some(pbbr), b"hello-world!"),
            ),
        ];
        let hashes: Vec<B256> = cases.iter().map(|(b, c)| hash_block_ctx(b, c)).collect();

        // Each case is deterministic and non-zero.
        for (i, (b, c)) in cases.iter().enumerate() {
            assert_eq!(
                hashes[i],
                hash_block_ctx(b, c),
                "case {i} non-deterministic"
            );
            assert!(!hashes[i].is_zero(), "case {i} unexpectedly zero");
        }
        // All 8 configurations distinct → every option/length boundary contributes.
        for i in 0..hashes.len() {
            for j in (i + 1)..hashes.len() {
                assert_ne!(hashes[i], hashes[j], "configs {i} vs {j} collide");
            }
        }

        // Per-field sensitivity: from a fully-populated baseline, perturbing each
        // scalar field must change the digest.
        let baseline_be = make_be(Some(prev), Some(blob));
        let baseline_ctx = make_ctx(Some(pbbr), b"hi");
        let baseline = hash_block_ctx(&baseline_be, &baseline_ctx);

        let perturb = |label: &str, mutator: &dyn Fn(&mut BlockEnv, &mut OpBlockExecutionCtx)| {
            let mut be = baseline_be.clone();
            let mut ctx = baseline_ctx.clone();
            mutator(&mut be, &mut ctx);
            assert_ne!(
                hash_block_ctx(&be, &ctx),
                baseline,
                "perturbing {label} must change digest",
            );
        };

        // BlockEnv scalar / option fields
        perturb("number", &|b, _| {
            b.number += U256::from(1u64);
        });
        perturb("beneficiary", &|b, _| {
            b.beneficiary = Address::from([0xFF; 20]);
        });
        perturb("timestamp", &|b, _| {
            b.timestamp += U256::from(1u64);
        });
        perturb("gas_limit", &|b, _| {
            b.gas_limit += 1;
        });
        perturb("basefee", &|b, _| {
            b.basefee += 1;
        });
        perturb("difficulty", &|b, _| {
            b.difficulty += U256::from(1u64);
        });
        perturb("prevrandao", &|b, _| {
            b.prevrandao = Some(B256::repeat_byte(0xEE));
        });
        perturb("blob.excess_blob_gas", &|b, _| {
            b.blob_excess_gas_and_price
                .as_mut()
                .unwrap()
                .excess_blob_gas += 1;
        });
        perturb("blob.blob_gasprice", &|b, _| {
            b.blob_excess_gas_and_price.as_mut().unwrap().blob_gasprice ^= 1;
        });

        // OpBlockExecutionCtx fields
        perturb("parent_hash", &|_, c| {
            c.parent_hash = B256::repeat_byte(0xEE);
        });
        perturb("parent_beacon_block_root", &|_, c| {
            c.parent_beacon_block_root = Some(B256::repeat_byte(0xEE));
        });
        perturb("extra_data", &|_, c| {
            c.extra_data = Bytes::from_static(b"different");
        });
    }

    // ---------- hash_results ----------

    /// One dense pass over `hash_results`. Covers determinism, all 3
    /// `ExecutionResult` variants, all 3 `SuccessReason` variants, all 6
    /// `OutOfGasError` variants, all 20 `HaltReason` variants (including
    /// `PrecompileErrorWithContext` payload sensitivity), `OpHaltReason::Base`
    /// vs `FailedDeposit`, both `Output` variants (`Call`, `Create` with
    /// `Some/None` address), logs (empty / single with topics+data / multiple),
    /// gas / gas_refunded / output / state field sensitivity, order sensitivity
    /// over (tx_hashes, results), `original_info` (pre-tx) authentication, and
    /// confirms transient fields (account-side `transaction_id`, slot-side
    /// `transaction_id` and `is_cold`) are dropped from the canonical encoding.
    /// Driving everything through `hash_results` mirrors how production code
    /// uses these helpers; the private `flatten_*` and `*_disc` helpers are
    /// exercised transitively.
    #[test]
    fn hash_results_dense() {
        // -- empty case --
        let h_empty = hash_results(&[], &[]);
        assert_eq!(hash_results(&[], &[]), h_empty, "empty deterministic");
        assert!(!h_empty.is_zero(), "empty hash is u64_be(0) hashed");

        // -- 3 ExecutionResult variants distinct, single-entry deterministic --
        let h_succ = hash_results(
            &[B256::ZERO],
            &[PartialResultAndState::from(stub_success(21_000))],
        );
        let h_rev = hash_results(
            &[B256::ZERO],
            &[PartialResultAndState::from(stub_revert(21_000, &[]))],
        );
        let h_halt = hash_results(
            &[B256::ZERO],
            &[PartialResultAndState::from(stub_halt(
                21_000,
                OpHaltReason::Base(HaltReason::OutOfGas(OutOfGasError::Basic)),
            ))],
        );
        assert_ne!(h_succ, h_empty, "Success differs from empty");
        assert_ne!(h_succ, h_rev, "Success vs Revert");
        assert_ne!(h_succ, h_halt, "Success vs Halt");
        assert_ne!(h_rev, h_halt, "Revert vs Halt");
        assert_eq!(
            h_succ,
            hash_results(
                &[B256::ZERO],
                &[PartialResultAndState::from(stub_success(21_000))],
            ),
            "single Success deterministic",
        );

        // -- order sensitivity over (tx_hashes, results) --
        let a = PartialResultAndState::from(stub_success(21_000));
        let b = PartialResultAndState::from(stub_success(42_000));
        let h_ab = hash_results(
            &[B256::repeat_byte(0x01), B256::repeat_byte(0x02)],
            &[a.clone(), b.clone()],
        );
        let h_ba = hash_results(&[B256::repeat_byte(0x02), B256::repeat_byte(0x01)], &[b, a]);
        assert_ne!(h_ab, h_ba, "swap order must change digest");

        // -- tx_hash sensitivity (results identical) --
        let single = PartialResultAndState::from(stub_success(21_000));
        assert_ne!(
            hash_results(&[B256::ZERO], std::slice::from_ref(&single)),
            hash_results(&[B256::repeat_byte(0xFF)], std::slice::from_ref(&single)),
            "tx_hash must bind into digest",
        );

        // -- gas_used (off-by-one) and gas_refunded sensitivity --
        assert_ne!(
            hash_results(
                &[B256::ZERO],
                &[PartialResultAndState::from(stub_success(21_000))],
            ),
            hash_results(
                &[B256::ZERO],
                &[PartialResultAndState::from(stub_success(21_001))],
            ),
        );
        let mut s_ref0 = stub_success(21_000);
        if let ExecutionResult::Success { gas_refunded, .. } = &mut s_ref0.result {
            *gas_refunded = 0;
        }
        let mut s_ref1 = stub_success(21_000);
        if let ExecutionResult::Success { gas_refunded, .. } = &mut s_ref1.result {
            *gas_refunded = 1;
        }
        assert_ne!(
            hash_results(&[B256::ZERO], &[PartialResultAndState::from(s_ref0)]),
            hash_results(&[B256::ZERO], &[PartialResultAndState::from(s_ref1)]),
            "gas_refunded must contribute",
        );

        // -- all SuccessReason variants distinguishable --
        let success_reason_hashes: Vec<B256> = all_success_reasons()
            .into_iter()
            .map(|r| {
                let mut ras = stub_success(21_000);
                if let ExecutionResult::Success { reason, .. } = &mut ras.result {
                    *reason = r;
                }
                hash_results(&[B256::ZERO], &[PartialResultAndState::from(ras)])
            })
            .collect();
        for i in 0..success_reason_hashes.len() {
            for j in (i + 1)..success_reason_hashes.len() {
                assert_ne!(
                    success_reason_hashes[i], success_reason_hashes[j],
                    "SuccessReason variants {i} vs {j} collide",
                );
            }
        }

        // -- all OutOfGasError variants distinguishable via Halt::Base(OutOfGas(*)) --
        let oog_hashes: Vec<B256> = all_oog_variants()
            .into_iter()
            .map(|oog| {
                hash_results(
                    &[B256::ZERO],
                    &[PartialResultAndState::from(stub_halt(
                        21_000,
                        OpHaltReason::Base(HaltReason::OutOfGas(oog)),
                    ))],
                )
            })
            .collect();
        for i in 0..oog_hashes.len() {
            for j in (i + 1)..oog_hashes.len() {
                assert_ne!(
                    oog_hashes[i], oog_hashes[j],
                    "OOG variants {i} vs {j} collide"
                );
            }
        }

        // -- all 20 HaltReason variants distinguishable via Halt::Base(*) --
        let halt_reasons = all_halt_reasons();
        let halt_hashes: Vec<B256> = halt_reasons
            .iter()
            .map(|hr| {
                hash_results(
                    &[B256::ZERO],
                    &[PartialResultAndState::from(stub_halt(
                        21_000,
                        OpHaltReason::Base(hr.clone()),
                    ))],
                )
            })
            .collect();
        for i in 0..halt_hashes.len() {
            for j in (i + 1)..halt_hashes.len() {
                assert_ne!(
                    halt_hashes[i], halt_hashes[j],
                    "HaltReason {:?} vs {:?} collide",
                    halt_reasons[i], halt_reasons[j],
                );
            }
        }

        // -- PrecompileErrorWithContext payload contributes --
        let h_pe1 = hash_results(
            &[B256::ZERO],
            &[PartialResultAndState::from(stub_halt(
                21_000,
                OpHaltReason::Base(HaltReason::PrecompileErrorWithContext("err1".to_string())),
            ))],
        );
        let h_pe2 = hash_results(
            &[B256::ZERO],
            &[PartialResultAndState::from(stub_halt(
                21_000,
                OpHaltReason::Base(HaltReason::PrecompileErrorWithContext("err2".to_string())),
            ))],
        );
        assert_ne!(
            h_pe1, h_pe2,
            "PrecompileErrorWithContext payload must contribute"
        );

        // -- OpHaltReason::FailedDeposit distinct from every Base(*) variant --
        let h_failed_deposit = hash_results(
            &[B256::ZERO],
            &[PartialResultAndState::from(stub_halt(
                21_000,
                OpHaltReason::FailedDeposit,
            ))],
        );
        for h in &halt_hashes {
            assert_ne!(
                h_failed_deposit, *h,
                "FailedDeposit collides with Base variant"
            );
        }

        // -- Output: Call, Create(None), Create(Some) all distinct --
        let mut out_call = stub_success(21_000);
        if let ExecutionResult::Success { output, .. } = &mut out_call.result {
            *output = Output::Call(Bytes::from_static(b"\xde\xad\xbe\xef"));
        }
        let mut out_create_none = stub_success(21_000);
        if let ExecutionResult::Success { output, .. } = &mut out_create_none.result {
            *output = Output::Create(Bytes::from_static(b"\x60\x80"), None);
        }
        let mut out_create_some = stub_success(21_000);
        if let ExecutionResult::Success { output, .. } = &mut out_create_some.result {
            *output = Output::Create(
                Bytes::from_static(b"\x60\x80"),
                Some(Address::from([0xCC; 20])),
            );
        }
        let h_call = hash_results(&[B256::ZERO], &[PartialResultAndState::from(out_call)]);
        let h_cn = hash_results(
            &[B256::ZERO],
            &[PartialResultAndState::from(out_create_none)],
        );
        let h_cs = hash_results(
            &[B256::ZERO],
            &[PartialResultAndState::from(out_create_some)],
        );
        assert_ne!(h_call, h_cn, "Call vs Create(None)");
        assert_ne!(h_call, h_cs, "Call vs Create(Some)");
        assert_ne!(h_cn, h_cs, "Create(None) vs Create(Some)");

        // -- Logs: empty vs single (with topics + data) vs multiple, plus topic sensitivity --
        let mut s_one_log = stub_success(21_000);
        if let ExecutionResult::Success { logs, .. } = &mut s_one_log.result {
            *logs = vec![log_with(
                Address::from([0xAA; 20]),
                vec![B256::repeat_byte(0x11), B256::repeat_byte(0x22)],
                b"data",
            )];
        }
        let mut s_multi_logs = stub_success(21_000);
        if let ExecutionResult::Success { logs, .. } = &mut s_multi_logs.result {
            *logs = vec![
                log_with(Address::from([0xAA; 20]), vec![B256::ZERO], b"a"),
                log_with(Address::from([0xBB; 20]), vec![], b""),
                log_with(
                    Address::from([0xCC; 20]),
                    vec![
                        B256::repeat_byte(0x11),
                        B256::repeat_byte(0x22),
                        B256::repeat_byte(0x33),
                    ],
                    b"longer-payload",
                ),
            ];
        }
        let h_one = hash_results(
            &[B256::ZERO],
            &[PartialResultAndState::from(s_one_log.clone())],
        );
        let h_multi = hash_results(&[B256::ZERO], &[PartialResultAndState::from(s_multi_logs)]);
        assert_ne!(h_succ, h_one, "log presence must change digest");
        assert_ne!(h_succ, h_multi);
        assert_ne!(h_one, h_multi, "log count must change digest");

        let mut s_diff_topic = stub_success(21_000);
        if let ExecutionResult::Success { logs, .. } = &mut s_diff_topic.result {
            *logs = vec![log_with(
                Address::from([0xAA; 20]),
                vec![B256::repeat_byte(0x99), B256::repeat_byte(0x22)], // first topic differs
                b"data",
            )];
        }
        let h_diff_topic =
            hash_results(&[B256::ZERO], &[PartialResultAndState::from(s_diff_topic)]);
        assert_ne!(h_one, h_diff_topic, "log topic change must propagate");

        // -- Revert output bytes sensitivity --
        assert_ne!(
            hash_results(
                &[B256::ZERO],
                &[PartialResultAndState::from(stub_revert(21_000, &[]))],
            ),
            hash_results(
                &[B256::ZERO],
                &[PartialResultAndState::from(stub_revert(21_000, b"err"))],
            ),
            "revert output bytes must contribute",
        );

        // ---- State diff binding: full per-field sensitivity sweep ----
        let addr_a = Address::from([0xA1; 20]);
        let addr_b = Address::from([0xB2; 20]);

        let mut ras_state = stub_success(21_000);
        ras_state.state.insert(
            addr_a,
            make_account(
                1,
                U256::from(1_000u64),
                0,
                AccountStatus::Touched,
                &[(U256::from(1u64), U256::ZERO, U256::from(42u64))],
            ),
        );
        ras_state.state.insert(
            addr_b,
            make_account(
                2,
                U256::from(2_000u64),
                1,
                AccountStatus::Touched | AccountStatus::Created,
                &[],
            ),
        );
        let h_state = hash_results(
            &[B256::ZERO],
            &[PartialResultAndState::from(ras_state.clone())],
        );
        assert_ne!(h_state, h_succ, "state diff must contribute");

        // address sensitivity
        let mut ras_diff_addr = ras_state.clone();
        let acc = ras_diff_addr.state.remove(&addr_a).unwrap();
        ras_diff_addr.state.insert(Address::from([0xA2; 20]), acc);
        assert_ne!(
            h_state,
            hash_results(&[B256::ZERO], &[PartialResultAndState::from(ras_diff_addr)]),
            "state-entry address must bind",
        );

        // info.{nonce, balance, code_hash} sensitivity
        let mut ras_nonce = ras_state.clone();
        ras_nonce.state.get_mut(&addr_a).unwrap().info.nonce = 99;
        assert_ne!(
            h_state,
            hash_results(&[B256::ZERO], &[PartialResultAndState::from(ras_nonce)]),
        );
        let mut ras_bal = ras_state.clone();
        ras_bal.state.get_mut(&addr_a).unwrap().info.balance = U256::from(99u64);
        assert_ne!(
            h_state,
            hash_results(&[B256::ZERO], &[PartialResultAndState::from(ras_bal)]),
        );
        let mut ras_code = ras_state.clone();
        ras_code.state.get_mut(&addr_a).unwrap().info.code_hash = B256::repeat_byte(0xCD);
        assert_ne!(
            h_state,
            hash_results(&[B256::ZERO], &[PartialResultAndState::from(ras_code)]),
        );

        // status bitflags
        let mut ras_status = ras_state.clone();
        ras_status.state.get_mut(&addr_a).unwrap().status =
            AccountStatus::Touched | AccountStatus::Created;
        assert_ne!(
            h_state,
            hash_results(&[B256::ZERO], &[PartialResultAndState::from(ras_status)]),
            "status bitflag change must propagate",
        );

        // Storage: present_value, original_value, additional slot, missing slot.
        let mut ras_slot = ras_state.clone();
        ras_slot.state.get_mut(&addr_a).unwrap().storage.insert(
            U256::from(1u64),
            EvmStorageSlot::new_changed(U256::ZERO, U256::from(43u64), 0),
        );
        assert_ne!(
            h_state,
            hash_results(&[B256::ZERO], &[PartialResultAndState::from(ras_slot)]),
            "present_value change must propagate",
        );
        let mut ras_orig_slot = ras_state.clone();
        ras_orig_slot
            .state
            .get_mut(&addr_a)
            .unwrap()
            .storage
            .insert(
                U256::from(1u64),
                EvmStorageSlot::new_changed(U256::from(7u64), U256::from(42u64), 0),
            );
        assert_ne!(
            h_state,
            hash_results(&[B256::ZERO], &[PartialResultAndState::from(ras_orig_slot)]),
            "original_value (pre-tx slot) change must propagate",
        );
        let mut ras_more_storage = ras_state.clone();
        ras_more_storage
            .state
            .get_mut(&addr_a)
            .unwrap()
            .storage
            .insert(
                U256::from(2u64),
                EvmStorageSlot::new_changed(U256::ZERO, U256::from(7u64), 0),
            );
        assert_ne!(
            h_state,
            hash_results(
                &[B256::ZERO],
                &[PartialResultAndState::from(ras_more_storage)]
            ),
            "additional storage entry must propagate",
        );

        // original_info contributes
        let mut ras_orig_info = ras_state.clone();
        *ras_orig_info.state.get_mut(&addr_a).unwrap().original_info = AccountInfo {
            nonce: 5,
            balance: U256::from(999u64),
            code_hash: B256::ZERO,
            account_id: None,
            code: None,
        };
        assert_ne!(
            h_state,
            hash_results(&[B256::ZERO], &[PartialResultAndState::from(ras_orig_info)]),
            "original_info (pre-tx view) must contribute",
        );

        // Transient fields must NOT contribute.
        let mut ras_transient = ras_state;
        let acct = ras_transient.state.get_mut(&addr_a).unwrap();
        acct.transaction_id = 999;
        let slot = acct.storage.get_mut(&U256::from(1u64)).unwrap();
        slot.transaction_id = 999;
        slot.is_cold = !slot.is_cold;
        assert_eq!(
            h_state,
            hash_results(&[B256::ZERO], &[PartialResultAndState::from(ras_transient)],),
            "transient fields must NOT contribute",
        );
    }

    /// Length-mismatch check: the explicit `assert_eq!` in `hash_results` is the
    /// last line of defense against an aggregator passing mismatched
    /// `(tx_hashes, results)` slices.
    #[test]
    #[should_panic(expected = "tx_hashes and results must have the same length")]
    fn hash_results_length_mismatch_panics() {
        hash_results(&[B256::ZERO], &[]);
    }

    // ---------- hash_expected_state ----------

    /// Empty determinism + multi-account + per-field sensitivity (exists,
    /// info.{nonce,balance,code_hash}, storage slot key/value, account/slot
    /// growth, address change). Walks `flatten_expected_state` and
    /// `flatten_expected_account` exhaustively.
    #[test]
    fn hash_expected_state_dense() {
        // Empty
        let h_empty = hash_expected_state(&[]);
        assert_eq!(hash_expected_state(&[]), h_empty, "empty deterministic");
        assert!(!h_empty.is_zero(), "empty hash is u64_be(0) hashed");

        // Production-shape baseline: 2 accounts, mixed exists, sorted by address.
        let addr_a = Address::from([0xA1; 20]);
        let addr_b = Address::from([0xB2; 20]);
        let baseline = vec![
            ExpectedStateEntry {
                address: addr_a,
                account: ExpectedAccount {
                    exists: true,
                    info: AccountInfo {
                        nonce: 1,
                        balance: U256::from(1_000u64),
                        code_hash: B256::repeat_byte(0x11),
                        account_id: None,
                        code: None,
                    },
                    storage: vec![
                        ExpectedStorageEntry {
                            slot: U256::from(1u64),
                            value: U256::from(7u64),
                        },
                        ExpectedStorageEntry {
                            slot: U256::from(2u64),
                            value: U256::from(8u64),
                        },
                    ],
                },
            },
            ExpectedStateEntry {
                address: addr_b,
                account: ExpectedAccount {
                    exists: false,
                    info: AccountInfo::default(),
                    storage: vec![],
                },
            },
        ];
        let h = hash_expected_state(&baseline);
        assert_eq!(hash_expected_state(&baseline), h, "deterministic");
        assert_ne!(h, h_empty, "non-empty must differ from empty");

        // exists flag — must affect digest (otherwise existence proofs collapse).
        let mut s = baseline.clone();
        s[1].account.exists = true;
        assert_ne!(h, hash_expected_state(&s), "exists flag must contribute");

        // info.{nonce, balance, code_hash} sensitivity
        let mut s = baseline.clone();
        s[0].account.info.nonce = 99;
        assert_ne!(h, hash_expected_state(&s), "expected info.nonce");
        let mut s = baseline.clone();
        s[0].account.info.balance = U256::from(2u64);
        assert_ne!(h, hash_expected_state(&s), "expected info.balance");
        let mut s = baseline.clone();
        s[0].account.info.code_hash = B256::repeat_byte(0x22);
        assert_ne!(h, hash_expected_state(&s), "expected info.code_hash");

        // Storage slot value / slot key sensitivity
        let mut s = baseline.clone();
        s[0].account.storage[0].value = U256::from(99u64);
        assert_ne!(h, hash_expected_state(&s), "storage value");
        let mut s = baseline.clone();
        s[0].account.storage[0].slot = U256::ZERO;
        assert_ne!(h, hash_expected_state(&s), "storage slot key");

        // Growing the storage / account vector must change the digest.
        let mut s = baseline.clone();
        s[0].account.storage.push(ExpectedStorageEntry {
            slot: U256::from(3u64),
            value: U256::from(9u64),
        });
        assert_ne!(h, hash_expected_state(&s), "additional storage slot");
        let mut s = baseline.clone();
        s.push(ExpectedStateEntry {
            address: Address::from([0xC3; 20]),
            account: ExpectedAccount {
                exists: true,
                info: AccountInfo::default(),
                storage: vec![],
            },
        });
        assert_ne!(h, hash_expected_state(&s), "additional account");

        // Address sensitivity (re-sort to keep the invariant intact).
        let mut s = baseline;
        s[0].address = Address::from([0xC4; 20]);
        s.sort_by_key(|e| e.address);
        assert_ne!(h, hash_expected_state(&s), "address change");
    }

    // ---------- Sorted-input invariants (debug_assert! panic paths) ----------
    //
    // Each of the 4 flatten_* functions that walks a sorted Vec asserts the
    // invariant via debug_assert!. These tests are necessarily isolated to
    // trigger one panic each; per-invariant `#[should_panic]` cannot be folded
    // into a parametric body.

    #[test]
    #[should_panic(expected = "flatten_partial_account: storage must be sorted by slot")]
    fn flatten_partial_account_unsorted_storage_panics() {
        let acct = PartialAccount {
            info: AccountInfo::default(),
            original_info: AccountInfo::default(),
            transaction_id: 0,
            status: AccountStatus::Touched,
            storage: vec![
                PartialStorageEntry {
                    slot: U256::from(2u64),
                    slot_value: EvmStorageSlot::new_changed(U256::ZERO, U256::ZERO, 0),
                },
                PartialStorageEntry {
                    slot: U256::from(1u64),
                    slot_value: EvmStorageSlot::new_changed(U256::ZERO, U256::ZERO, 0),
                },
            ],
        };
        let _ = flatten_partial_account(&acct);
    }

    #[test]
    #[should_panic(expected = "flatten_expected_account: storage must be sorted by slot")]
    fn flatten_expected_account_unsorted_storage_panics() {
        let acct = ExpectedAccount {
            exists: true,
            info: AccountInfo::default(),
            storage: vec![
                ExpectedStorageEntry {
                    slot: U256::from(2u64),
                    value: U256::ZERO,
                },
                ExpectedStorageEntry {
                    slot: U256::from(1u64),
                    value: U256::ZERO,
                },
            ],
        };
        let _ = flatten_expected_account(&acct);
    }

    #[test]
    #[should_panic(expected = "flatten_expected_state: state must be sorted by address")]
    fn flatten_expected_state_unsorted_address_panics() {
        let s = vec![
            ExpectedStateEntry {
                address: Address::from([0xB2; 20]),
                account: ExpectedAccount {
                    exists: true,
                    info: AccountInfo::default(),
                    storage: vec![],
                },
            },
            ExpectedStateEntry {
                address: Address::from([0xA1; 20]),
                account: ExpectedAccount {
                    exists: true,
                    info: AccountInfo::default(),
                    storage: vec![],
                },
            },
        ];
        let _ = flatten_expected_state(&s);
    }

    #[test]
    #[should_panic(expected = "flatten_partial_state: state must be sorted by address")]
    fn flatten_partial_state_unsorted_address_panics() {
        let acct = PartialAccount {
            info: AccountInfo::default(),
            original_info: AccountInfo::default(),
            transaction_id: 0,
            status: AccountStatus::Touched,
            storage: vec![],
        };
        let s = vec![
            PartialStateEntry {
                address: Address::from([0xB2; 20]),
                account: acct.clone(),
            },
            PartialStateEntry {
                address: Address::from([0xA1; 20]),
                account: acct,
            },
        ];
        let _ = flatten_partial_state(&s);
    }
}

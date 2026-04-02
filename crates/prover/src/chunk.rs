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

//! Host-side chunk witness construction.
//!
//! This module provides utilities for splitting block transactions into independently
//! provable chunks and constructing the [`ChunkWitnessData`] instances needed for chunk
//! proving. The [`ChunkWitnessData`] type itself lives in `kailua-kona` (shared with
//! the guest); this module contains only the host-side construction logic.

use std::collections::BTreeMap;
use std::ops::Range;

use alloy_evm::revm::context::BlockEnv;
use alloy_evm::revm::database::in_memory_db::{AccountState, Cache, DbAccount};
use alloy_evm::revm::primitives::KECCAK_EMPTY;
use alloy_evm::revm::state::{AccountInfo, AccountStatus, EvmState};
use alloy_op_evm::block::OpBlockExecutionCtx;
use alloy_primitives::{Address, Bytes, B256, U256};
use op_alloy_consensus::OpReceiptEnvelope;

use kailua_kona::precondition::chunking::EvmAccumulatorState;
use kailua_kona::witness::ChunkWitnessData;

/// Groups `tx_count` transactions into sequential, non-overlapping chunks of at most
/// `max_txs_per_chunk` transactions each. The last chunk may have fewer transactions.
///
/// # Panics
///
/// Panics if `max_txs_per_chunk == 0`.
pub fn group_transactions_into_chunks(
    tx_count: usize,
    max_txs_per_chunk: usize,
) -> Vec<Range<usize>> {
    assert!(max_txs_per_chunk > 0, "max_txs_per_chunk must be positive");
    (0..tx_count)
        .step_by(max_txs_per_chunk)
        .map(|start| start..tx_count.min(start + max_txs_per_chunk))
        .collect()
}

/// Per-transaction metadata needed during chunk witness construction that is not recoverable
/// from `EvmState` traces alone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChunkTxMeta {
    /// Exact block hashes read by this transaction's tx body.
    pub block_hashes: BTreeMap<U256, B256>,
    /// DA footprint added by this transaction.
    pub da_footprint_delta: u64,
    /// Blob gas used by this transaction.
    pub blob_gas_used_delta: u64,
}

// -- Witness construction --

/// Maps the bitflags-based [`AccountStatus`] (from `EvmState` traces) to the enum-based
/// [`AccountState`] (from `Cache`/`CacheDB`).
fn account_state_from_evm_status(status: AccountStatus) -> AccountState {
    if status.intersects(AccountStatus::SelfDestructed | AccountStatus::Created) {
        AccountState::StorageCleared
    } else if status.contains(AccountStatus::Touched) {
        AccountState::Touched
    } else {
        AccountState::None
    }
}

/// Applies a single transaction's [`EvmState`] trace to the cumulative cache,
/// updating account info, storage, lifecycle state, and contract bytecodes.
fn apply_trace_to_cache(cache: &mut Cache, trace: &EvmState) {
    for (addr, account) in trace {
        let db_account = cache.accounts.entry(*addr).or_insert_with(|| DbAccount {
            info: AccountInfo::default(),
            account_state: AccountState::NotExisting,
            storage: Default::default(),
        });

        db_account.info = account.info.clone();
        db_account.account_state = account_state_from_evm_status(account.status);

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

/// Pre-populates the cumulative cache with all accounts, contracts, and block hashes
/// that any chunk in the block will access. Addresses absent from the post-prelude cache
/// are inserted as [`AccountState::NotExisting`]. This ensures every chunk's witness
/// carries the full cumulative state at its boundary, which is required for hash chain
/// continuity (`post_db_hash[i] == pre_db_hash[i+1]`).
fn prepare_cumulative_cache(
    post_prelude_cache: &Cache,
    all_traces: &[EvmState],
    all_tx_meta: &[ChunkTxMeta],
) -> Cache {
    let mut cache = post_prelude_cache.clone();

    for trace in all_traces {
        for (addr, account) in trace {
            // Ensure every accessed address exists (NotExisting for absent)
            cache
                .accounts
                .entry(*addr)
                .or_insert_with(DbAccount::new_not_existing);

            // Pre-populate contract bytecodes from traces (pre-existing, not Created)
            if account.info.code_hash != KECCAK_EMPTY
                && !account.status.contains(AccountStatus::Created)
            {
                if let Some(code) = &account.info.code {
                    cache
                        .contracts
                        .entry(account.info.code_hash)
                        .or_insert_with(|| code.clone());
                }
            }
        }
    }

    // Pre-populate all block hashes from tx metadata
    for meta in all_tx_meta {
        for (num, hash) in &meta.block_hashes {
            cache.block_hashes.entry(*num).or_insert(*hash);
        }
    }

    cache
}

/// Accumulates a receipt into the EVM accumulator state at a chunk boundary.
fn accumulate_receipt(
    state: &mut EvmAccumulatorState,
    receipt: &OpReceiptEnvelope,
    tx_meta: &ChunkTxMeta,
) {
    state.cumulative_gas_used = receipt.cumulative_gas_used();
    state.da_footprint_used = state
        .da_footprint_used
        .saturating_add(tx_meta.da_footprint_delta);
    state.blob_gas_used = state
        .blob_gas_used
        .saturating_add(tx_meta.blob_gas_used_delta);
    // Accrue log bloom from receipt logs
    if let Some(inner) = receipt.as_receipt() {
        for log in &inner.logs {
            state.logs_bloom.accrue_log(log);
        }
    }
    state.receipts.push(receipt.clone());
}

/// Builds [`ChunkWitnessData`] instances for each transaction chunk in a block.
///
/// For each chunk, this function:
/// 1. Clones the full cumulative cache state as the chunk's pre-state snapshot
/// 2. Records the EVM accumulator state at the chunk boundary
/// 3. Advances the cumulative state through the chunk's traces for the next chunk
///
/// Each chunk receives the **full** cumulative cache (not a filtered subset) so that
/// `hash(chunk_i post-state) == hash(chunk_{i+1} pre-state)` — the hash chain invariant.
///
/// # Panics
///
/// Panics if `traces.len() != block_txs.len()` or `traces.len() != receipts.len()`.
#[allow(clippy::too_many_arguments)]
pub fn build_chunk_witnesses(
    traces: &[EvmState],
    tx_meta: &[ChunkTxMeta],
    post_prelude_cache: &Cache,
    block_txs: &[Bytes],
    receipts: &[OpReceiptEnvelope],
    max_txs_per_chunk: usize,
    block_env: &BlockEnv,
    op_block_ctx: &OpBlockExecutionCtx,
    evm_state_after_prelude: EvmAccumulatorState,
    agreed_l2_output_root: B256,
    config_hash: B256,
    fpvm_image_id: B256,
    payout_recipient: Address,
) -> Vec<ChunkWitnessData> {
    assert_eq!(traces.len(), block_txs.len());
    assert_eq!(tx_meta.len(), block_txs.len());
    assert_eq!(traces.len(), receipts.len());

    let chunks = group_transactions_into_chunks(block_txs.len(), max_txs_per_chunk);
    let total_chunks = chunks.len() as u16;

    let mut cumulative_cache = prepare_cumulative_cache(post_prelude_cache, traces, tx_meta);
    let mut cumulative_evm_state = evm_state_after_prelude;
    let mut witnesses = Vec::with_capacity(chunks.len());

    for (chunk_idx, chunk_range) in chunks.iter().enumerate() {
        let chunk_cache = cumulative_cache.clone();

        witnesses.push(ChunkWitnessData {
            block_number: block_env.number.to::<u64>(),
            chunk_index: chunk_idx as u16,
            total_chunks,
            tx_start: chunk_range.start as u16,
            tx_count: chunk_range.len() as u16,
            transactions: block_txs[chunk_range.clone()]
                .iter()
                .map(|tx| tx.to_vec())
                .collect(),
            block_env: block_env.clone(),
            op_block_ctx: op_block_ctx.clone(),
            cache: chunk_cache,
            evm_state: cumulative_evm_state.clone(),
            agreed_l2_output_root,
            config_hash,
            fpvm_image_id,
            payout_recipient,
        });

        // Advance cumulative state through this chunk's transactions
        for tx_idx in chunk_range.clone() {
            apply_trace_to_cache(&mut cumulative_cache, &traces[tx_idx]);
            for (num, hash) in &tx_meta[tx_idx].block_hashes {
                cumulative_cache.block_hashes.insert(*num, *hash);
            }
            accumulate_receipt(
                &mut cumulative_evm_state,
                &receipts[tx_idx],
                &tx_meta[tx_idx],
            );
        }
    }

    witnesses
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_evm::revm::context_interface::block::BlobExcessGasAndPrice;
    use alloy_evm::revm::state::{Account, Bytecode, EvmStorageSlot};
    use alloy_primitives::{address, map::HashMap};
    use op_alloy_consensus::OpTxType;
    use std::collections::BTreeMap;

    // -- group_transactions_into_chunks tests --

    #[test]
    fn even_division() {
        let chunks = group_transactions_into_chunks(12, 4);
        assert_eq!(chunks, vec![0..4, 4..8, 8..12]);
    }

    #[test]
    fn remainder_chunk() {
        let chunks = group_transactions_into_chunks(10, 4);
        assert_eq!(chunks, vec![0..4, 4..8, 8..10]);
    }

    #[test]
    fn single_chunk_when_max_ge_total() {
        let chunks = group_transactions_into_chunks(5, 100);
        assert_eq!(chunks, vec![0..5]);
    }

    #[test]
    fn single_transaction_per_chunk() {
        let chunks = group_transactions_into_chunks(3, 1);
        assert_eq!(chunks, vec![0..1, 1..2, 2..3]);
    }

    #[test]
    fn zero_transactions() {
        let chunks = group_transactions_into_chunks(0, 4);
        assert!(chunks.is_empty());
    }

    #[test]
    fn exact_single_chunk() {
        let chunks = group_transactions_into_chunks(4, 4);
        assert_eq!(chunks, vec![0..4]);
    }

    #[test]
    #[should_panic(expected = "max_txs_per_chunk must be positive")]
    fn zero_chunk_size_rejected() {
        group_transactions_into_chunks(10, 0);
    }

    // -- Helpers for build_chunk_witnesses tests --

    fn make_account(nonce: u64, balance: u64, storage: Vec<(U256, U256)>) -> Account {
        let mut evm_storage: HashMap<U256, EvmStorageSlot> = Default::default();
        for (k, v) in &storage {
            evm_storage.insert(
                *k,
                EvmStorageSlot {
                    original_value: U256::ZERO,
                    present_value: *v,
                    transaction_id: 0,
                    is_cold: false,
                },
            );
        }
        Account {
            info: AccountInfo {
                nonce,
                balance: U256::from(balance),
                code_hash: KECCAK_EMPTY,
                code: None,
            },
            transaction_id: 0,
            storage: evm_storage,
            status: AccountStatus::Touched,
        }
    }

    fn make_db_account(nonce: u64, balance: u64, storage: Vec<(U256, U256)>) -> DbAccount {
        let mut slot_map: HashMap<U256, U256> = Default::default();
        for (k, v) in storage {
            slot_map.insert(k, v);
        }
        DbAccount {
            info: AccountInfo {
                nonce,
                balance: U256::from(balance),
                code_hash: KECCAK_EMPTY,
                code: None,
            },
            account_state: AccountState::Touched,
            storage: slot_map,
        }
    }

    fn make_receipt(cumulative_gas: u64) -> OpReceiptEnvelope {
        OpReceiptEnvelope::from_parts(true, cumulative_gas, vec![], OpTxType::Legacy, None, None)
    }

    fn default_block_env() -> BlockEnv {
        BlockEnv {
            number: U256::from(100),
            beneficiary: Address::ZERO,
            timestamp: U256::from(1000),
            gas_limit: 30_000_000,
            basefee: 1,
            difficulty: U256::ZERO,
            prevrandao: None,
            blob_excess_gas_and_price: None,
        }
    }

    fn default_op_block_ctx() -> OpBlockExecutionCtx {
        OpBlockExecutionCtx {
            parent_hash: B256::ZERO,
            parent_beacon_block_root: None,
            extra_data: Bytes::new(),
        }
    }

    fn default_tx_meta(count: usize) -> Vec<ChunkTxMeta> {
        vec![ChunkTxMeta::default(); count]
    }

    // -- BlockEnv / OpBlockExecutionCtx rkyv round-trip tests --

    #[test]
    fn block_env_rkyv_round_trip() {
        use kailua_kona::rkyv::chunking::BlockEnvRkyv;
        use kailua_kona::{from_bytes_with, to_bytes_with};

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
        assert_eq!(deser.prevrandao, env.prevrandao);
        assert_eq!(
            deser.blob_excess_gas_and_price,
            env.blob_excess_gas_and_price
        );
    }

    #[test]
    fn op_block_ctx_rkyv_round_trip() {
        use kailua_kona::rkyv::chunking::OpBlockExecutionCtxRkyv;
        use kailua_kona::{from_bytes_with, to_bytes_with};

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

    // -- build_chunk_witnesses tests --

    #[test]
    fn single_chunk_witness_matches_prelude_state() {
        let addr = address!("0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        let mut post_prelude = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        post_prelude.accounts.insert(
            addr,
            make_db_account(1, 1000, vec![(U256::from(1), U256::from(100))]),
        );

        let trace: EvmState = [(
            addr,
            make_account(2, 900, vec![(U256::from(1), U256::from(200))]),
        )]
        .into_iter()
        .collect();
        let txs = vec![Bytes::from_static(&[0xAA])];
        let receipts = vec![make_receipt(21000)];

        let witnesses = build_chunk_witnesses(
            &[trace],
            &default_tx_meta(1),
            &post_prelude,
            &txs,
            &receipts,
            100, // all in one chunk
            &default_block_env(),
            &default_op_block_ctx(),
            EvmAccumulatorState::default(),
            B256::ZERO,
            B256::ZERO,
            B256::ZERO,
            Address::ZERO,
        );

        assert_eq!(witnesses.len(), 1);
        let w = &witnesses[0];
        assert_eq!(w.chunk_index, 0);
        assert_eq!(w.total_chunks, 1);
        assert_eq!(w.tx_start, 0);
        assert_eq!(w.tx_count, 1);
        // Cache should contain addr with pre-state (from post_prelude)
        let acct = w.cache.accounts.get(&addr).unwrap();
        assert_eq!(acct.info.nonce, 1);
        assert_eq!(acct.info.balance, U256::from(1000));
        assert_eq!(acct.storage.get(&U256::from(1)), Some(&U256::from(100)));
    }

    #[test]
    fn two_chunk_storage_carry_forward() {
        let addr = address!("0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        let slot = U256::from(42);

        let mut post_prelude = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        post_prelude
            .accounts
            .insert(addr, make_db_account(0, 1000, vec![(slot, U256::from(0))]));

        let trace0: EvmState = [(addr, make_account(1, 900, vec![(slot, U256::from(999))]))]
            .into_iter()
            .collect();
        let trace1: EvmState = [(addr, make_account(2, 800, vec![(slot, U256::from(999))]))]
            .into_iter()
            .collect();

        let txs = vec![Bytes::from_static(&[0x01]), Bytes::from_static(&[0x02])];
        let receipts = vec![make_receipt(21000), make_receipt(42000)];

        let witnesses = build_chunk_witnesses(
            &[trace0, trace1],
            &default_tx_meta(2),
            &post_prelude,
            &txs,
            &receipts,
            1,
            &default_block_env(),
            &default_op_block_ctx(),
            EvmAccumulatorState::default(),
            B256::ZERO,
            B256::ZERO,
            B256::ZERO,
            Address::ZERO,
        );

        assert_eq!(witnesses.len(), 2);
        assert_eq!(
            witnesses[0].cache.accounts.get(&addr).unwrap().storage[&slot],
            U256::from(0)
        );
        assert_eq!(
            witnesses[1].cache.accounts.get(&addr).unwrap().storage[&slot],
            U256::from(999)
        );
    }

    #[test]
    fn nonce_balance_carry_forward() {
        let sender = address!("0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");

        let mut post_prelude = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        post_prelude
            .accounts
            .insert(sender, make_db_account(5, 10000, vec![]));

        let trace0: EvmState = [(sender, make_account(6, 9000, vec![]))]
            .into_iter()
            .collect();
        let trace1: EvmState = [(sender, make_account(7, 8000, vec![]))]
            .into_iter()
            .collect();

        let txs = vec![Bytes::from_static(&[0x01]), Bytes::from_static(&[0x02])];
        let receipts = vec![make_receipt(21000), make_receipt(42000)];

        let witnesses = build_chunk_witnesses(
            &[trace0, trace1],
            &default_tx_meta(2),
            &post_prelude,
            &txs,
            &receipts,
            1,
            &default_block_env(),
            &default_op_block_ctx(),
            EvmAccumulatorState::default(),
            B256::ZERO,
            B256::ZERO,
            B256::ZERO,
            Address::ZERO,
        );

        let w0_acct = witnesses[0].cache.accounts.get(&sender).unwrap();
        assert_eq!(w0_acct.info.nonce, 5);
        assert_eq!(w0_acct.info.balance, U256::from(10000));

        let w1_acct = witnesses[1].cache.accounts.get(&sender).unwrap();
        assert_eq!(w1_acct.info.nonce, 6);
        assert_eq!(w1_acct.info.balance, U256::from(9000));
    }

    #[test]
    fn contract_creation_carry_forward() {
        let creator = address!("0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        let contract = address!("0xBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB");
        let code = Bytecode::new_raw(Bytes::from_static(&[0x60, 0x00]));
        let code_hash = code.hash_slow();

        let mut post_prelude = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        post_prelude
            .accounts
            .insert(creator, make_db_account(0, 10000, vec![]));

        let mut created_account = make_account(1, 0, vec![]);
        created_account.info.code_hash = code_hash;
        created_account.info.code = Some(code.clone());
        created_account.status = AccountStatus::Created | AccountStatus::Touched;
        let trace0: EvmState = [
            (creator, make_account(1, 9000, vec![])),
            (contract, created_account),
        ]
        .into_iter()
        .collect();

        let mut call_account = make_account(1, 0, vec![]);
        call_account.info.code_hash = code_hash;
        call_account.info.code = Some(code.clone());
        let trace1: EvmState = [
            (creator, make_account(2, 8000, vec![])),
            (contract, call_account),
        ]
        .into_iter()
        .collect();

        let txs = vec![Bytes::from_static(&[0x01]), Bytes::from_static(&[0x02])];
        let receipts = vec![make_receipt(100000), make_receipt(200000)];

        let witnesses = build_chunk_witnesses(
            &[trace0, trace1],
            &default_tx_meta(2),
            &post_prelude,
            &txs,
            &receipts,
            1,
            &default_block_env(),
            &default_op_block_ctx(),
            EvmAccumulatorState::default(),
            B256::ZERO,
            B256::ZERO,
            B256::ZERO,
            Address::ZERO,
        );

        assert_eq!(
            witnesses[0]
                .cache
                .accounts
                .get(&contract)
                .unwrap()
                .account_state,
            AccountState::NotExisting
        );

        // Chunk 1: contract carried forward from chunk 0
        let w1_contract = witnesses[1].cache.accounts.get(&contract).unwrap();
        assert_eq!(w1_contract.info.code_hash, code_hash);
        assert_eq!(w1_contract.account_state, AccountState::StorageCleared);
        assert!(witnesses[1].cache.contracts.contains_key(&code_hash));
        assert_eq!(
            witnesses[1]
                .cache
                .contracts
                .get(&code_hash)
                .unwrap()
                .original_bytes(),
            code.original_bytes()
        );
    }

    #[test]
    fn selfdestruct_carry_forward() {
        let addr = address!("0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        let slot = U256::from(1);

        let mut post_prelude = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        post_prelude
            .accounts
            .insert(addr, make_db_account(1, 5000, vec![(slot, U256::from(42))]));

        let mut destroyed = make_account(1, 0, vec![]);
        destroyed.status = AccountStatus::SelfDestructed | AccountStatus::Touched;
        let trace0: EvmState = [(addr, destroyed)].into_iter().collect();
        let trace1: EvmState = [(addr, make_account(0, 0, vec![]))].into_iter().collect();

        let txs = vec![Bytes::from_static(&[0x01]), Bytes::from_static(&[0x02])];
        let receipts = vec![make_receipt(21000), make_receipt(42000)];

        let witnesses = build_chunk_witnesses(
            &[trace0, trace1],
            &default_tx_meta(2),
            &post_prelude,
            &txs,
            &receipts,
            1,
            &default_block_env(),
            &default_op_block_ctx(),
            EvmAccumulatorState::default(),
            B256::ZERO,
            B256::ZERO,
            B256::ZERO,
            Address::ZERO,
        );

        assert!(witnesses[0].cache.accounts.contains_key(&addr));
        assert_eq!(
            witnesses[0]
                .cache
                .accounts
                .get(&addr)
                .unwrap()
                .account_state,
            AccountState::Touched
        );

        let w1_acct = witnesses[1].cache.accounts.get(&addr).unwrap();
        assert_eq!(w1_acct.account_state, AccountState::StorageCleared);
        assert!(w1_acct.storage.is_empty());
    }

    #[test]
    fn block_hash_inclusion() {
        let addr = address!("0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");

        let mut post_prelude = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        post_prelude
            .accounts
            .insert(addr, make_db_account(0, 0, vec![]));

        let trace: EvmState = [(addr, make_account(0, 0, vec![]))].into_iter().collect();
        let txs = vec![Bytes::from_static(&[0x01])];
        let receipts = vec![make_receipt(21000)];

        let witnesses = build_chunk_witnesses(
            &[trace],
            &[ChunkTxMeta {
                block_hashes: BTreeMap::from([(U256::from(99), B256::repeat_byte(0xFF))]),
                ..Default::default()
            }],
            &post_prelude,
            &txs,
            &receipts,
            100,
            &default_block_env(),
            &default_op_block_ctx(),
            EvmAccumulatorState::default(),
            B256::ZERO,
            B256::ZERO,
            B256::ZERO,
            Address::ZERO,
        );

        assert_eq!(
            witnesses[0].cache.block_hashes.get(&U256::from(99)),
            Some(&B256::repeat_byte(0xFF))
        );
    }

    #[test]
    fn evm_accumulator_continuity() {
        let addr = address!("0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");

        let mut post_prelude = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        post_prelude
            .accounts
            .insert(addr, make_db_account(0, 10000, vec![]));

        let trace0: EvmState = [(addr, make_account(1, 9000, vec![]))]
            .into_iter()
            .collect();
        let trace1: EvmState = [(addr, make_account(2, 8000, vec![]))]
            .into_iter()
            .collect();

        let txs = vec![Bytes::from_static(&[0x01]), Bytes::from_static(&[0x02])];
        let receipts = vec![make_receipt(21000), make_receipt(42000)];

        let base_evm = EvmAccumulatorState {
            cumulative_gas_used: 10000,
            ..Default::default()
        };

        let witnesses = build_chunk_witnesses(
            &[trace0, trace1],
            &[
                ChunkTxMeta {
                    da_footprint_delta: 500,
                    blob_gas_used_delta: 131072,
                    ..Default::default()
                },
                ChunkTxMeta {
                    da_footprint_delta: 250,
                    blob_gas_used_delta: 0,
                    ..Default::default()
                },
            ],
            &post_prelude,
            &txs,
            &receipts,
            1,
            &default_block_env(),
            &default_op_block_ctx(),
            base_evm,
            B256::ZERO,
            B256::ZERO,
            B256::ZERO,
            Address::ZERO,
        );

        assert_eq!(witnesses[0].evm_state.cumulative_gas_used, 10000);
        assert!(witnesses[0].evm_state.receipts.is_empty());

        assert_eq!(witnesses[1].evm_state.cumulative_gas_used, 21000);
        assert_eq!(witnesses[1].evm_state.da_footprint_used, 500);
        assert_eq!(witnesses[1].evm_state.blob_gas_used, 131072);
        assert_eq!(witnesses[1].evm_state.receipts.len(), 1);
    }

    #[test]
    fn missing_account_is_materialized_as_not_existing() {
        let absent = address!("0xCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC");
        let trace: EvmState = [(absent, make_account(0, 0, vec![]))].into_iter().collect();
        let txs = vec![Bytes::from_static(&[0xAA])];
        let receipts = vec![make_receipt(21000)];

        let witnesses = build_chunk_witnesses(
            &[trace],
            &default_tx_meta(1),
            &Cache {
                accounts: Default::default(),
                contracts: Default::default(),
                logs: Vec::new(),
                block_hashes: Default::default(),
            },
            &txs,
            &receipts,
            1,
            &default_block_env(),
            &default_op_block_ctx(),
            EvmAccumulatorState::default(),
            B256::ZERO,
            B256::ZERO,
            B256::ZERO,
            Address::ZERO,
        );

        let account = witnesses[0].cache.accounts.get(&absent).unwrap();
        assert_eq!(account.account_state, AccountState::NotExisting);
    }

    #[test]
    fn first_chunk_includes_preexisting_contract_bytecode_from_trace() {
        let contract = address!("0xBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB");
        let code = Bytecode::new_raw(Bytes::from_static(&[0x60, 0x00]));
        let code_hash = code.hash_slow();

        let mut post_prelude = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        post_prelude.accounts.insert(
            contract,
            DbAccount {
                info: AccountInfo {
                    nonce: 1,
                    balance: U256::ZERO,
                    code_hash,
                    code: None,
                },
                account_state: AccountState::None,
                storage: Default::default(),
            },
        );

        let mut touched_contract = make_account(1, 0, vec![]);
        touched_contract.info.code_hash = code_hash;
        touched_contract.info.code = Some(code.clone());
        let trace: EvmState = [(contract, touched_contract)].into_iter().collect();
        let txs = vec![Bytes::from_static(&[0x01])];
        let receipts = vec![make_receipt(21000)];

        let witnesses = build_chunk_witnesses(
            &[trace],
            &default_tx_meta(1),
            &post_prelude,
            &txs,
            &receipts,
            1,
            &default_block_env(),
            &default_op_block_ctx(),
            EvmAccumulatorState::default(),
            B256::ZERO,
            B256::ZERO,
            B256::ZERO,
            Address::ZERO,
        );

        assert_eq!(
            witnesses[0]
                .cache
                .contracts
                .get(&code_hash)
                .unwrap()
                .original_bytes(),
            code.original_bytes()
        );
    }

    #[test]
    fn hash_chain_continuity_across_chunks() {
        use kailua_kona::precondition::chunking::hash_cache;

        let addr = address!("0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        let slot = U256::from(1);

        let mut post_prelude = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        post_prelude
            .accounts
            .insert(addr, make_db_account(0, 10000, vec![(slot, U256::from(0))]));

        // Tx 0 (chunk 0): writes slot 1 = 42, nonce 0→1, balance 10000→9000
        let trace0: EvmState = [(addr, make_account(1, 9000, vec![(slot, U256::from(42))]))]
            .into_iter()
            .collect();
        // Tx 1 (chunk 1): writes slot 1 = 99, nonce 1→2, balance 9000→8000
        let trace1: EvmState = [(addr, make_account(2, 8000, vec![(slot, U256::from(99))]))]
            .into_iter()
            .collect();

        let txs = vec![Bytes::from_static(&[0x01]), Bytes::from_static(&[0x02])];
        let receipts = vec![make_receipt(21000), make_receipt(42000)];

        let witnesses = build_chunk_witnesses(
            &[trace0.clone(), trace1],
            &default_tx_meta(2),
            &post_prelude,
            &txs,
            &receipts,
            1,
            &default_block_env(),
            &default_op_block_ctx(),
            EvmAccumulatorState::default(),
            B256::ZERO,
            B256::ZERO,
            B256::ZERO,
            Address::ZERO,
        );

        assert_eq!(witnesses.len(), 2);

        // Simulate what the chunk 0 guest would compute as post_db_hash:
        // Start with chunk 0's witness cache, apply chunk 0's trace, hash the result.
        let mut chunk0_post_cache = witnesses[0].cache.clone();
        apply_trace_to_cache(&mut chunk0_post_cache, &trace0);
        let chunk0_post_hash = hash_cache(&chunk0_post_cache);

        // Chunk 1's pre_db_hash is just the hash of its witness cache.
        let chunk1_pre_hash = hash_cache(&witnesses[1].cache);

        // Hash chain invariant: post_db_hash[0] == pre_db_hash[1]
        assert_eq!(
            chunk0_post_hash, chunk1_pre_hash,
            "hash chain broken: chunk 0 post-hash != chunk 1 pre-hash"
        );
    }
}

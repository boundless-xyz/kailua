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

//! Transaction chunk proving support.
//!
//! This module provides utilities for splitting block transactions into independently
//! provable chunks and constructing the witnesses needed for chunk proving and aggregation.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;

use alloy_evm::revm::context::BlockEnv;
use alloy_evm::revm::context_interface::block::BlobExcessGasAndPrice;
use alloy_evm::revm::database::in_memory_db::{AccountState, Cache, DbAccount};
use alloy_evm::revm::primitives::KECCAK_EMPTY;
use alloy_evm::revm::state::{AccountInfo, AccountStatus, EvmState};
use alloy_op_evm::block::OpBlockExecutionCtx;
use alloy_primitives::{Address, Bytes, B256, U256};
use op_alloy_consensus::OpReceiptEnvelope;

use crate::precondition::chunking::EvmAccumulatorState;
use crate::rkyv::chunking::{CacheRkyv, EvmAccumulatorStateRkyv};
use crate::rkyv::primitives::{AddressDef, B256Def};

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

/// Block execution context required by chunk proving.
///
/// Captures the fields from [`BlockEnv`] and [`OpBlockExecutionCtx`] needed to
/// reconstruct the execution environment inside the chunk prover.
#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ChunkBlockContext {
    // BlockEnv fields
    pub number: u64,
    #[rkyv(with = AddressDef)]
    pub beneficiary: Address,
    pub timestamp: u64,
    pub gas_limit: u64,
    pub basefee: u64,
    #[rkyv(with = rkyv::with::Map<B256Def>)]
    pub prevrandao: Option<B256>,
    pub blob_excess_gas: Option<u64>,
    pub blob_gasprice: Option<u128>,
    // OpBlockExecutionCtx fields
    #[rkyv(with = B256Def)]
    pub parent_hash: B256,
    #[rkyv(with = rkyv::with::Map<B256Def>)]
    pub parent_beacon_block_root: Option<B256>,
    pub extra_data: Vec<u8>,
}

impl ChunkBlockContext {
    /// Creates a `ChunkBlockContext` from the revm block environment and OP execution context.
    pub fn new(block_env: &BlockEnv, ctx: &OpBlockExecutionCtx) -> Self {
        Self {
            number: block_env.number.to::<u64>(),
            beneficiary: block_env.beneficiary,
            timestamp: block_env.timestamp.to::<u64>(),
            gas_limit: block_env.gas_limit,
            basefee: block_env.basefee,
            prevrandao: block_env.prevrandao,
            blob_excess_gas: block_env
                .blob_excess_gas_and_price
                .as_ref()
                .map(|b| b.excess_blob_gas),
            blob_gasprice: block_env
                .blob_excess_gas_and_price
                .as_ref()
                .map(|b| b.blob_gasprice),
            parent_hash: ctx.parent_hash,
            parent_beacon_block_root: ctx.parent_beacon_block_root,
            extra_data: ctx.extra_data.to_vec(),
        }
    }

    /// Reconstructs a [`BlockEnv`] from this context.
    pub fn to_block_env(&self) -> BlockEnv {
        BlockEnv {
            number: U256::from(self.number),
            beneficiary: self.beneficiary,
            timestamp: U256::from(self.timestamp),
            gas_limit: self.gas_limit,
            basefee: self.basefee,
            difficulty: U256::ZERO,
            prevrandao: self.prevrandao,
            blob_excess_gas_and_price: self.blob_excess_gas.map(|excess| BlobExcessGasAndPrice {
                excess_blob_gas: excess,
                blob_gasprice: self.blob_gasprice.unwrap_or(0),
            }),
        }
    }

    /// Reconstructs an [`OpBlockExecutionCtx`] from this context.
    pub fn to_op_block_execution_ctx(&self) -> OpBlockExecutionCtx {
        OpBlockExecutionCtx {
            parent_hash: self.parent_hash,
            parent_beacon_block_root: self.parent_beacon_block_root,
            extra_data: Bytes::from(self.extra_data.clone()),
        }
    }
}

/// Witness data for proving a single transaction chunk within a block.
///
/// Contains the pre-chunk state snapshot, transaction data, and metadata
/// required for the chunk prover to re-execute the chunk in isolation.
#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ChunkWitness {
    pub block_number: u64,
    pub chunk_index: u16,
    pub total_chunks: u16,
    pub tx_start: u16,
    pub tx_count: u16,
    pub transactions: Vec<Vec<u8>>,
    pub block_context: ChunkBlockContext,
    #[rkyv(with = CacheRkyv)]
    pub cache: Cache,
    #[rkyv(with = EvmAccumulatorStateRkyv)]
    pub evm_state: EvmAccumulatorState,
    #[rkyv(with = B256Def)]
    pub agreed_l2_output_root: B256,
    #[rkyv(with = B256Def)]
    pub config_hash: B256,
    #[rkyv(with = B256Def)]
    pub fpvm_image_id: B256,
    #[rkyv(with = AddressDef)]
    pub payout_recipient: Address,
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

/// Builds a filtered [`Cache`] containing only the state that a chunk's transactions
/// will access, as determined by the chunk's traces.
fn build_chunk_cache(
    cumulative: &Cache,
    chunk_traces: &[EvmState],
    chunk_meta: &[ChunkTxMeta],
) -> Cache {
    // Collect all addresses and storage slots accessed by this chunk
    let mut needed_addrs: HashSet<Address> = HashSet::new();
    let mut needed_slots: HashMap<Address, HashSet<U256>> = HashMap::new();
    let mut needed_block_hashes: BTreeMap<U256, B256> = BTreeMap::new();
    let mut trace_contracts = HashMap::new();
    for trace in chunk_traces {
        for (addr, account) in trace {
            needed_addrs.insert(*addr);
            let slots = needed_slots.entry(*addr).or_default();
            for slot in account.storage.keys() {
                slots.insert(*slot);
            }
            if account.info.code_hash != KECCAK_EMPTY
                && !account.status.contains(AccountStatus::Created)
            {
                if let Some(code) = &account.info.code {
                    trace_contracts
                        .entry(account.info.code_hash)
                        .or_insert_with(|| code.clone());
                }
            }
        }
    }
    for meta in chunk_meta {
        for (num, hash) in &meta.block_hashes {
            needed_block_hashes.insert(*num, *hash);
        }
    }

    let mut cache = Cache {
        accounts: Default::default(),
        contracts: Default::default(),
        logs: Vec::new(),
        block_hashes: needed_block_hashes.into_iter().collect(),
    };

    // Populate accounts from cumulative state
    for addr in &needed_addrs {
        if let Some(db_account) = cumulative.accounts.get(addr) {
            let mut account = db_account.clone();
            // Filter storage to only slots this chunk accesses
            if let Some(slots) = needed_slots.get(addr) {
                account.storage.retain(|k, _| slots.contains(k));
            } else {
                account.storage.clear();
            }
            // Include contract bytecode if account has code
            if db_account.info.code_hash != KECCAK_EMPTY {
                if let Some(code) = cumulative
                    .contracts
                    .get(&db_account.info.code_hash)
                    .or(db_account.info.code.as_ref())
                {
                    cache
                        .contracts
                        .insert(db_account.info.code_hash, code.clone());
                }
            }
            cache.accounts.insert(*addr, account);
        } else {
            cache.accounts.insert(*addr, DbAccount::new_not_existing());
        }
    }

    // Include any additional contract bytecode surfaced by the tx-body traces (for example
    // pre-existing contracts first called in chunk 0 whose bytecode was not preloaded into the
    // post-prelude cache's contracts map).
    for (code_hash, code) in trace_contracts {
        cache.contracts.entry(code_hash).or_insert(code);
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

/// Builds [`ChunkWitness`] instances for each transaction chunk in a block.
///
/// For each chunk, this function:
/// 1. Captures the cumulative cache state as the chunk's pre-state snapshot
/// 2. Filters the snapshot to include only state the chunk's transactions access
/// 3. Records the EVM accumulator state at the chunk boundary
/// 4. Advances the cumulative state through the chunk's traces for the next chunk
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
    block_context: ChunkBlockContext,
    evm_state_after_prelude: EvmAccumulatorState,
    agreed_l2_output_root: B256,
    config_hash: B256,
    fpvm_image_id: B256,
    payout_recipient: Address,
) -> Vec<ChunkWitness> {
    assert_eq!(traces.len(), block_txs.len());
    assert_eq!(tx_meta.len(), block_txs.len());
    assert_eq!(traces.len(), receipts.len());

    let chunks = group_transactions_into_chunks(block_txs.len(), max_txs_per_chunk);
    let total_chunks = chunks.len() as u16;

    let mut cumulative_cache = post_prelude_cache.clone();
    let mut cumulative_evm_state = evm_state_after_prelude;
    let mut witnesses = Vec::with_capacity(chunks.len());

    for (chunk_idx, chunk_range) in chunks.iter().enumerate() {
        // Build this chunk's filtered cache from the current cumulative state
        let chunk_cache = build_chunk_cache(
            &cumulative_cache,
            &traces[chunk_range.clone()],
            &tx_meta[chunk_range.clone()],
        );

        witnesses.push(ChunkWitness {
            block_number: block_context.number,
            chunk_index: chunk_idx as u16,
            total_chunks,
            tx_start: chunk_range.start as u16,
            tx_count: chunk_range.len() as u16,
            transactions: block_txs[chunk_range.clone()]
                .iter()
                .map(|tx| tx.to_vec())
                .collect(),
            block_context: block_context.clone(),
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

    fn default_block_context() -> ChunkBlockContext {
        ChunkBlockContext {
            number: 100,
            beneficiary: Address::ZERO,
            timestamp: 1000,
            gas_limit: 30_000_000,
            basefee: 1,
            prevrandao: None,
            blob_excess_gas: None,
            blob_gasprice: None,
            parent_hash: B256::ZERO,
            parent_beacon_block_root: None,
            extra_data: Vec::new(),
        }
    }

    fn default_tx_meta(count: usize) -> Vec<ChunkTxMeta> {
        vec![ChunkTxMeta::default(); count]
    }

    // -- ChunkBlockContext tests --

    #[test]
    fn block_context_round_trip() {
        let block_env = BlockEnv {
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
        let ctx = OpBlockExecutionCtx {
            parent_hash: B256::repeat_byte(0xBB),
            parent_beacon_block_root: Some(B256::repeat_byte(0xCC)),
            extra_data: Bytes::from_static(&[1, 2, 3]),
        };

        let chunk_ctx = ChunkBlockContext::new(&block_env, &ctx);
        let restored_env = chunk_ctx.to_block_env();
        let restored_ctx = chunk_ctx.to_op_block_execution_ctx();

        assert_eq!(restored_env.number, block_env.number);
        assert_eq!(restored_env.beneficiary, block_env.beneficiary);
        assert_eq!(restored_env.timestamp, block_env.timestamp);
        assert_eq!(restored_env.gas_limit, block_env.gas_limit);
        assert_eq!(restored_env.basefee, block_env.basefee);
        assert_eq!(restored_env.prevrandao, block_env.prevrandao);
        assert_eq!(
            restored_env.blob_excess_gas_and_price,
            block_env.blob_excess_gas_and_price
        );
        assert_eq!(restored_ctx.parent_hash, ctx.parent_hash);
        assert_eq!(
            restored_ctx.parent_beacon_block_root,
            ctx.parent_beacon_block_root
        );
        assert_eq!(restored_ctx.extra_data, ctx.extra_data);
    }

    #[test]
    fn block_context_rkyv_round_trip() {
        // ChunkBlockContext derives rkyv directly, test round-trip
        let ctx = default_block_context();
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&ctx)
            .unwrap()
            .to_vec();
        let deser: ChunkBlockContext =
            rkyv::from_bytes::<ChunkBlockContext, rkyv::rancor::Error>(&bytes).unwrap();
        assert_eq!(deser.number, ctx.number);
        assert_eq!(deser.timestamp, ctx.timestamp);
        assert_eq!(deser.gas_limit, ctx.gas_limit);
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
            default_block_context(),
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

        // Tx 0 (chunk 0): writes slot 42 = 999
        let trace0: EvmState = [(addr, make_account(1, 900, vec![(slot, U256::from(999))]))]
            .into_iter()
            .collect();
        // Tx 1 (chunk 1): reads slot 42 (should see 999)
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
            1, // 1 tx per chunk
            default_block_context(),
            EvmAccumulatorState::default(),
            B256::ZERO,
            B256::ZERO,
            B256::ZERO,
            Address::ZERO,
        );

        assert_eq!(witnesses.len(), 2);

        // Chunk 0: pre-state from post_prelude (slot=0)
        let w0 = &witnesses[0];
        assert_eq!(
            w0.cache.accounts.get(&addr).unwrap().storage[&slot],
            U256::from(0)
        );

        // Chunk 1: pre-state carries forward chunk 0's write (slot=999)
        let w1 = &witnesses[1];
        assert_eq!(
            w1.cache.accounts.get(&addr).unwrap().storage[&slot],
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

        // Tx 0 (chunk 0): sender nonce 5→6, balance 10000→9000
        let trace0: EvmState = [(sender, make_account(6, 9000, vec![]))]
            .into_iter()
            .collect();
        // Tx 1 (chunk 1): sender nonce 6→7, balance 9000→8000
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
            default_block_context(),
            EvmAccumulatorState::default(),
            B256::ZERO,
            B256::ZERO,
            B256::ZERO,
            Address::ZERO,
        );

        // Chunk 0: nonce=5, balance=10000
        let w0_acct = witnesses[0].cache.accounts.get(&sender).unwrap();
        assert_eq!(w0_acct.info.nonce, 5);
        assert_eq!(w0_acct.info.balance, U256::from(10000));

        // Chunk 1: nonce=6, balance=9000 (carried forward from chunk 0)
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

        // Tx 0 (chunk 0): creates contract
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

        // Tx 1 (chunk 1): calls the contract
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
            default_block_context(),
            EvmAccumulatorState::default(),
            B256::ZERO,
            B256::ZERO,
            B256::ZERO,
            Address::ZERO,
        );

        // Chunk 0: contract is explicit non-existence pre-state
        assert_eq!(
            witnesses[0]
                .cache
                .accounts
                .get(&contract)
                .unwrap()
                .account_state,
            AccountState::NotExisting
        );
        assert!(!witnesses[0].cache.contracts.contains_key(&code_hash));

        // Chunk 1: contract carried forward from chunk 0
        let w1_contract = witnesses[1].cache.accounts.get(&contract).unwrap();
        assert_eq!(w1_contract.info.code_hash, code_hash);
        assert_eq!(w1_contract.account_state, AccountState::StorageCleared);
        // Contract bytecode included
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

        // Tx 0 (chunk 0): self-destructs the account
        let mut destroyed = make_account(1, 0, vec![]);
        destroyed.status = AccountStatus::SelfDestructed | AccountStatus::Touched;
        let trace0: EvmState = [(addr, destroyed)].into_iter().collect();

        // Tx 1 (chunk 1): touches the same address
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
            default_block_context(),
            EvmAccumulatorState::default(),
            B256::ZERO,
            B256::ZERO,
            B256::ZERO,
            Address::ZERO,
        );

        // Chunk 0: pre-state has the account (storage not included since trace has no slot access)
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

        // Chunk 1: account is StorageCleared, storage wiped
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
            default_block_context(),
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
            default_block_context(),
            base_evm,
            B256::ZERO,
            B256::ZERO,
            B256::ZERO,
            Address::ZERO,
        );

        // Chunk 0: starts with base EVM state
        assert_eq!(witnesses[0].evm_state.cumulative_gas_used, 10000);
        assert!(witnesses[0].evm_state.receipts.is_empty());

        // Chunk 1: accumulated through chunk 0's receipt
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
            default_block_context(),
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
            default_block_context(),
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
}

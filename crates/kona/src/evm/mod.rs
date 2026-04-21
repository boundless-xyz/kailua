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

use crate::precondition::chunking::EvmAccumulatorState;
use crate::rkyv::chunking::{BlockEnvRkyv, OpBlockExecutionCtxRkyv, ResultAndStateRkyv};
use crate::rkyv::primitives::B256Def;
use alloy_evm::op_revm::OpHaltReason;
use alloy_evm::revm::context::result::ResultAndState;
use alloy_evm::revm::context::BlockEnv;
use alloy_op_evm::OpBlockExecutionCtx;
use alloy_primitives::B256;

pub mod cached;
pub mod db;
pub mod tracing;

/// Represents a proven transaction subsequence within a block.
#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct PartialExecution {
    /// Hash of the DB state before this chunk's transactions.
    #[rkyv(with = B256Def)]
    pub agreed_db: B256,
    /// Hash of the EVM accumulator state before this chunk's transactions.
    #[rkyv(with = B256Def)]
    pub agreed_evm: B256,
    /// Hash of the chunk's transaction list.
    #[rkyv(with = B256Def)]
    pub tx_hash: B256,
    /// Full per-tx execution results (ExecutionResult + EvmState), in order.
    #[rkyv(with = rkyv::with::Map<ResultAndStateRkyv>)]
    pub results: Vec<ResultAndState<OpHaltReason>>,
    /// EVM accumulator state at the chunk boundary (post-execution).
    pub evm_state: EvmAccumulatorState,
    /// Hash of the DB state after this chunk's transactions.
    #[rkyv(with = B256Def)]
    pub claimed_db: B256,
    /// Hash of the EVM accumulator state after this chunk's transactions.
    #[rkyv(with = B256Def)]
    pub claimed_evm: B256,
    /// Block execution `BlockEnv` under which this chunk's transactions executed
    /// (timestamp, basefee, prevrandao, coinbase, blob pricing, etc.).
    #[rkyv(with = BlockEnvRkyv)]
    pub block_env: BlockEnv,
    /// OP block execution context (parent_hash for BLOCKHASH / EIP-2935,
    /// parent_beacon_block_root for EIP-4788, extra_data for Holocene/Jovian
    /// EIP-1559 params).
    #[rkyv(with = OpBlockExecutionCtxRkyv)]
    pub op_block_ctx: OpBlockExecutionCtx,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::cached::CachedEvmFactory;
    use crate::evm::PartialExecution;
    use crate::precondition::chunking::EvmAccumulatorState;
    use alloy_evm::op_revm::{OpHaltReason, OpSpecId, OpTransaction};
    use alloy_evm::revm::context::CfgEnv;
    use alloy_evm::revm::context::{BlockEnv, TxEnv};
    use alloy_evm::revm::context_interface::result::ResultAndState;
    use alloy_evm::revm::context_interface::result::{ExecutionResult, Output, SuccessReason};
    use alloy_evm::revm::database::in_memory_db::InMemoryDB;
    use alloy_evm::revm::state::AccountInfo;
    use alloy_evm::{Evm, EvmEnv, EvmFactory};
    use alloy_primitives::Address;
    use alloy_primitives::{address, keccak256, TxKind, B256, U256};
    use std::collections::HashMap;

    fn test_env_for_block(block_number: u64) -> EvmEnv<OpSpecId> {
        let block_env = BlockEnv {
            number: U256::from(block_number),
            ..Default::default()
        };
        let mut cfg_env = CfgEnv::default();
        cfg_env.chain_id = 1;
        cfg_env.spec = OpSpecId::BEDROCK;
        EvmEnv { block_env, cfg_env }
    }

    fn make_transfer(
        caller: Address,
        to: Address,
        value: U256,
        nonce: u64,
    ) -> OpTransaction<TxEnv> {
        OpTransaction {
            base: TxEnv {
                caller,
                kind: TxKind::Call(to),
                value,
                gas_limit: 21000,
                gas_price: 1,
                nonce,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Builds a stub `ResultAndState` that flags itself with a recognizable gas_used so
    /// tests can confirm the pre-computed entry — not live execution — was returned.
    fn stub_result_and_state(gas_used: u64) -> ResultAndState<OpHaltReason> {
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

    fn stub_chunk(tag: u16, gas_used_markers: &[u64]) -> PartialExecution {
        PartialExecution {
            agreed_db: keccak256(format!("agreed_db_{tag}")),
            agreed_evm: keccak256(format!("agreed_evm_{tag}")),
            tx_hash: keccak256(format!("tx_hash_{tag}")),
            results: gas_used_markers
                .iter()
                .copied()
                .map(stub_result_and_state)
                .collect(),
            evm_state: EvmAccumulatorState::default(),
            claimed_db: keccak256(format!("claimed_db_{tag}")),
            claimed_evm: keccak256(format!("claimed_evm_{tag}")),
            block_env: alloy_evm::revm::context::BlockEnv::default(),
            op_block_ctx: alloy_op_evm::block::OpBlockExecutionCtx::default(),
        }
    }

    /// A pre-computed chunk entry must be returned verbatim by `transact_raw()`,
    /// bypassing inner-EVM execution. The marker gas_used proves it came from the chunk.
    #[test]
    fn precomputed_results_returned_in_order() {
        let chunks = HashMap::from([(1u64, vec![stub_chunk(0, &[100_001, 100_002, 100_003])])]);
        let factory = CachedEvmFactory::new(chunks);

        let sender = address!("0x1000000000000000000000000000000000000000");
        let recipient = address!("0x2000000000000000000000000000000000000000");
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            sender,
            AccountInfo {
                balance: U256::from(1_000_000_000_000_000_000u128),
                nonce: 0,
                ..Default::default()
            },
        );

        let mut evm = factory.create_evm(db, test_env_for_block(1));

        for expected in [100_001u64, 100_002, 100_003] {
            let result = evm
                .transact_raw(make_transfer(sender, recipient, U256::from(1), 0))
                .unwrap();
            match result.result {
                ExecutionResult::Success { gas_used, .. } => assert_eq!(gas_used, expected),
                _ => panic!("expected pre-computed Success result"),
            }
            // Pre-computed state is empty — a live transfer would diff two accounts.
            assert!(
                result.state.is_empty(),
                "pre-computed state should be empty"
            );
        }
    }

    /// Blocks without chunk entries flow through the inner `OpEvm` normally: every
    /// `transact_raw()` delegates, and the state diff carries real account changes.
    #[test]
    fn empty_chunks_delegate_to_inner() {
        let factory = CachedEvmFactory::new(HashMap::new());

        let sender = address!("0x1000000000000000000000000000000000000000");
        let recipient = address!("0x2000000000000000000000000000000000000000");
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            sender,
            AccountInfo {
                balance: U256::from(1_000_000_000_000_000_000u128),
                nonce: 0,
                ..Default::default()
            },
        );

        let mut evm = factory.create_evm(db, test_env_for_block(1));
        let result = evm
            .transact_raw(make_transfer(sender, recipient, U256::from(1000), 0))
            .unwrap();

        // Live execution must populate both accounts in state.
        assert!(result.state.contains_key(&sender));
        assert!(result.state.contains_key(&recipient));
    }

    /// System calls bypass chunk routing: they must delegate to the inner EVM and leave
    /// the tx index untouched so the next tx-body transaction still receives chunk
    /// result index 0.
    #[test]
    fn system_calls_delegate_and_do_not_advance_tx_index() {
        let chunks = HashMap::from([(1u64, vec![stub_chunk(0, &[42])])]);
        let factory = CachedEvmFactory::new(chunks);

        let db = InMemoryDB::default();
        let mut evm = factory.create_evm(db, test_env_for_block(1));

        // Simulate a prelude system call — tx_index must stay at 0.
        let caller = address!("0xdeaddeaddeaddeaddeaddeaddeaddeaddead0001");
        let contract = address!("0x000F3df6D732807Ef1319fB7B8bB8522d0Beac02");
        let _ = evm.transact_system_call(caller, contract, alloy_primitives::Bytes::new());

        // Now the first tx-body transaction should still get the chunk's result[0].
        let sender = address!("0x1000000000000000000000000000000000000000");
        let result = evm
            .transact_raw(make_transfer(sender, Address::ZERO, U256::ZERO, 0))
            .unwrap();
        match result.result {
            ExecutionResult::Success { gas_used, .. } => assert_eq!(gas_used, 42),
            _ => panic!("expected pre-computed Success result"),
        }
    }

    /// Multiple chunks within a block are consumed sequentially, with the cursor
    /// advancing across chunk boundaries.
    #[test]
    fn multi_chunk_within_block_crosses_boundary() {
        // Chunk 0: txs 0..2, markers [10, 20]. Chunk 1: txs 2..4, markers [30, 40].
        let chunks = HashMap::from([(
            1u64,
            vec![stub_chunk(0, &[10, 20]), stub_chunk(2, &[30, 40])],
        )]);
        let factory = CachedEvmFactory::new(chunks);

        let db = InMemoryDB::default();
        let mut evm = factory.create_evm(db, test_env_for_block(1));

        for expected in [10u64, 20, 30, 40] {
            let sender = address!("0x1000000000000000000000000000000000000000");
            let result = evm
                .transact_raw(make_transfer(sender, Address::ZERO, U256::ZERO, 0))
                .unwrap();
            match result.result {
                ExecutionResult::Success { gas_used, .. } => assert_eq!(gas_used, expected),
                _ => panic!("expected pre-computed Success result"),
            }
        }
    }

    /// `create_evm` is keyed by `input.block_env.number`: block N's chunks must not leak
    /// into block M's factory call.
    #[test]
    fn factory_routes_by_block_number() {
        // Only block 5 has chunks.
        let chunks = HashMap::from([(5u64, vec![stub_chunk(0, &[777])])]);
        let factory = CachedEvmFactory::new(chunks);

        // create_evm for block 7 → no chunks → delegate to inner.
        let sender = address!("0x1000000000000000000000000000000000000000");
        let mut db7 = InMemoryDB::default();
        db7.insert_account_info(
            sender,
            AccountInfo {
                balance: U256::from(1_000_000_000_000_000_000u128),
                nonce: 0,
                ..Default::default()
            },
        );
        let mut evm7 = factory.create_evm(db7, test_env_for_block(7));
        let r7 = evm7
            .transact_raw(make_transfer(sender, Address::ZERO, U256::from(1), 0))
            .unwrap();
        // Live execution — non-empty state.
        assert!(!r7.state.is_empty());

        // create_evm for block 5 → returns the pre-computed result.
        let db5 = InMemoryDB::default();
        let mut evm5 = factory.create_evm(db5, test_env_for_block(5));
        let r5 = evm5
            .transact_raw(make_transfer(sender, Address::ZERO, U256::from(1), 0))
            .unwrap();
        match r5.result {
            ExecutionResult::Success { gas_used, .. } => assert_eq!(gas_used, 777),
            _ => panic!("expected pre-computed Success result"),
        }
    }

    /// Trait-method field access should flow through the default `db()`/`block()` etc.
    /// methods and reach the inner `OpEvm`.
    #[test]
    fn trait_method_field_access_works() {
        let factory = CachedEvmFactory::new(HashMap::new());
        let db = InMemoryDB::default();
        let evm = factory.create_evm(db, test_env_for_block(42));
        assert_eq!(evm.block().number, U256::from(42));
        assert_eq!(evm.chain_id(), 1);
        let _ = evm.db();
    }

    /// `take_chunks` is destructive: a second `create_evm` for the same block number
    /// returns a ChunkingEvm with an empty chunk vec (delegates to inner).
    #[test]
    fn take_chunks_is_destructive_per_block() {
        let chunks = HashMap::from([(9u64, vec![stub_chunk(0, &[123])])]);
        let factory = CachedEvmFactory::new(chunks);

        // First call drains the chunks for block 9.
        let drained = factory.take_chunks(9);
        assert_eq!(drained.len(), 1);
        // Second call returns empty.
        assert!(factory.take_chunks(9).is_empty());
    }

    /// When a block's chunks are exhausted, further `transact_raw()` calls delegate to
    /// the inner EVM. This supports graceful fall-through when a block has more tx-body
    /// calls than chunk results (defensive — in production the host covers every tx).
    #[test]
    fn exhausted_chunks_delegate_to_inner() {
        // Chunk has only one result.
        let chunks = HashMap::from([(1u64, vec![stub_chunk(0, &[999])])]);
        let factory = CachedEvmFactory::new(chunks);

        let sender = address!("0x1000000000000000000000000000000000000000");
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            sender,
            AccountInfo {
                balance: U256::from(1_000_000_000_000_000_000u128),
                nonce: 0,
                ..Default::default()
            },
        );
        let mut evm = factory.create_evm(db, test_env_for_block(1));

        // tx 0: pre-computed.
        let r0 = evm
            .transact_raw(make_transfer(sender, Address::ZERO, U256::from(1), 0))
            .unwrap();
        match r0.result {
            ExecutionResult::Success { gas_used, .. } => assert_eq!(gas_used, 999),
            _ => panic!("expected pre-computed Success result"),
        }
        assert!(r0.state.is_empty(), "pre-computed state should be empty");

        // tx 1: chunks exhausted, delegate — real state diff.
        let r1 = evm
            .transact_raw(make_transfer(sender, Address::ZERO, U256::from(1), 0))
            .unwrap();
        assert!(!r1.state.is_empty(), "delegated tx should have state diff");

        // Silence unused warnings from helper imports.
        let _ = B256::ZERO;
    }
}

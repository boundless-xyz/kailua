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

pub mod cached;
pub mod expected;
pub mod partial;
pub mod witness;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use crate::evm::cached::CachedEvmFactory;
    use crate::evm::expected::{
        ExpectedAccount, ExpectedStateEntry, ExpectedStorageEntry, EXPECTED_STORAGE_SLOTS,
    };
    use crate::evm::partial::{PartialExecution, PartialResultAndState};
    use crate::evm::witness::PartialExecutionWitness;
    use alloy_evm::op_revm::{constants::L1_BLOCK_CONTRACT, OpHaltReason, OpSpecId, OpTransaction};
    use alloy_evm::revm::context::CfgEnv;
    use alloy_evm::revm::context::{BlockEnv, TxEnv};
    use alloy_evm::revm::context_interface::result::ResultAndState;
    use alloy_evm::revm::context_interface::result::{ExecutionResult, Output, SuccessReason};
    use alloy_evm::revm::database::in_memory_db::InMemoryDB;
    use alloy_evm::revm::database::states::CacheAccount;
    use alloy_evm::revm::database::CacheState;
    use alloy_evm::revm::primitives::HashMap as RevmHashMap;
    use alloy_evm::revm::state::Account;
    use alloy_evm::revm::state::AccountInfo;
    use alloy_evm::revm::state::AccountStatus;
    use alloy_evm::revm::state::Bytecode;
    use alloy_evm::revm::state::EvmStorageSlot;
    use alloy_evm::revm::DatabaseCommit;
    use alloy_evm::{Evm, EvmEnv, EvmFactory};
    use alloy_op_evm::block::OpBlockExecutionCtx;
    use alloy_primitives::Address;
    use alloy_primitives::{address, keccak256, TxKind, U256};
    use alloy_primitives::{Bytes, B256};

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

    fn stub_result_with_l1_block_slot_change(
        gas_used: u64,
        slot: U256,
        original_value: U256,
        present_value: U256,
    ) -> PartialResultAndState {
        let mut storage: RevmHashMap<U256, EvmStorageSlot> = Default::default();
        storage.insert(
            slot,
            EvmStorageSlot::new_changed(original_value, present_value, 0),
        );

        let info = AccountInfo::default();
        let account = Account {
            info: info.clone(),
            original_info: Box::new(info),
            transaction_id: 0,
            storage,
            status: AccountStatus::Touched,
        };

        let mut state = RevmHashMap::default();
        state.insert(L1_BLOCK_CONTRACT, account);

        PartialResultAndState::from(ResultAndState {
            result: ExecutionResult::Success {
                reason: SuccessReason::Return,
                gas_used,
                gas_refunded: 0,
                logs: vec![],
                output: Output::Call(alloy_primitives::Bytes::new()),
            },
            state,
        })
    }

    fn stub_expected_state() -> Vec<ExpectedStateEntry> {
        vec![ExpectedStateEntry {
            address: L1_BLOCK_CONTRACT,
            account: ExpectedAccount {
                exists: true,
                info: AccountInfo::default(),
                storage: EXPECTED_STORAGE_SLOTS
                    .into_iter()
                    .map(|slot| ExpectedStorageEntry {
                        slot,
                        value: U256::ZERO,
                    })
                    .collect(),
            },
        }]
    }

    fn insert_stub_expected_state(db: &mut InMemoryDB) {
        db.insert_account_info(L1_BLOCK_CONTRACT, AccountInfo::default());
        for slot in EXPECTED_STORAGE_SLOTS {
            db.insert_account_storage(L1_BLOCK_CONTRACT, slot, U256::ZERO)
                .unwrap();
        }
    }

    fn stub_chunk(block_number: u64, gas_used_markers: &[u64]) -> PartialExecution {
        let default_tx_hash = keccak256([0x00u8]);
        let block_env = BlockEnv {
            number: U256::from(block_number),
            ..Default::default()
        };
        PartialExecution {
            tx_hashes: vec![default_tx_hash; gas_used_markers.len()],
            results: gas_used_markers
                .iter()
                .copied()
                .map(stub_result_and_state)
                .map(PartialResultAndState::from)
                .collect(),
            expected_state: stub_expected_state(),
            block_env,
            op_block_ctx: alloy_op_evm::block::OpBlockExecutionCtx::default(),
        }
    }

    #[test]
    fn precomputed_results_returned_in_order() {
        let chunks = vec![vec![stub_chunk(1, &[100_001, 100_002, 100_003])]];
        let factory = CachedEvmFactory::new(chunks);

        let sender = address!("0x1000000000000000000000000000000000000000");
        let recipient = address!("0x2000000000000000000000000000000000000000");
        let mut db = InMemoryDB::default();
        insert_stub_expected_state(&mut db);
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

    #[test]
    #[should_panic(expected = "ExpectedState validation failure.")]
    fn cached_chunk_rejects_missing_expected_state() {
        let mut chunk = stub_chunk(1, &[42]);
        chunk.expected_state.clear();
        let factory = CachedEvmFactory::new(vec![vec![chunk]]);

        let db = InMemoryDB::default();
        let mut evm = factory.create_evm(db, test_env_for_block(1));

        let sender = address!("0x1000000000000000000000000000000000000000");
        let _ = evm
            .transact_raw(make_transfer(sender, Address::ZERO, U256::ZERO, 0))
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "ExpectedState validation failure.")]
    fn cached_chunk_rejects_expected_state_mismatch() {
        let factory = CachedEvmFactory::new(vec![vec![stub_chunk(1, &[42])]]);

        let mut db = InMemoryDB::default();
        insert_stub_expected_state(&mut db);
        db.insert_account_storage(L1_BLOCK_CONTRACT, EXPECTED_STORAGE_SLOTS[0], U256::from(1))
            .unwrap();
        let mut evm = factory.create_evm(db, test_env_for_block(1));

        let sender = address!("0x1000000000000000000000000000000000000000");
        let _ = evm
            .transact_raw(make_transfer(sender, Address::ZERO, U256::ZERO, 0))
            .unwrap();
    }

    #[test]
    fn cached_chunk_verifies_expected_state_only_on_first_use() {
        let changed_slot = EXPECTED_STORAGE_SLOTS[0];
        let mut chunk = stub_chunk(1, &[10, 20]);
        chunk.results[0] =
            stub_result_with_l1_block_slot_change(10, changed_slot, U256::ZERO, U256::from(1));
        let factory = CachedEvmFactory::new(vec![vec![chunk]]);

        let mut db = InMemoryDB::default();
        insert_stub_expected_state(&mut db);
        let mut evm = factory.create_evm(db, test_env_for_block(1));

        let sender = address!("0x1000000000000000000000000000000000000000");
        let first = evm
            .transact_raw(make_transfer(sender, Address::ZERO, U256::ZERO, 0))
            .unwrap();
        evm.db_mut().commit(first.state);

        // This would fail if the chunk's initial expected_state were rechecked
        // after the first cached result had advanced the live DB.
        let second = evm
            .transact_raw(make_transfer(sender, Address::ZERO, U256::ZERO, 0))
            .unwrap();
        match second.result {
            ExecutionResult::Success { gas_used, .. } => assert_eq!(gas_used, 20),
            _ => panic!("expected pre-computed Success result"),
        }
    }

    #[test]
    fn empty_chunks_delegate_to_inner() {
        let factory = CachedEvmFactory::new(Vec::new());

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

    #[test]
    fn system_calls_delegate_and_do_not_advance_tx_index() {
        let chunks = vec![vec![stub_chunk(1, &[42])]];
        let factory = CachedEvmFactory::new(chunks);

        let mut db = InMemoryDB::default();
        insert_stub_expected_state(&mut db);
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

    #[test]
    fn multi_chunk_within_block_crosses_boundary() {
        // Chunk 0: txs 0..2, markers [10, 20]. Chunk 1: txs 2..4, markers [30, 40].
        let chunks = vec![vec![stub_chunk(1, &[10, 20]), stub_chunk(1, &[30, 40])]];
        let factory = CachedEvmFactory::new(chunks);

        let mut db = InMemoryDB::default();
        insert_stub_expected_state(&mut db);
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

    #[test]
    fn factory_serves_chunks_in_creation_order() {
        // Slot 0: empty (first EVM created gets no pre-computed results).
        // Slot 1: one chunk with marker 777 (second EVM gets the pre-computed result).
        // Second EVM is built with `test_env_for_block(5)`, so the chunk's
        // `block_env.number` must equal 5 to satisfy `CachedEvm::transact_raw`.
        let chunks = vec![vec![], vec![stub_chunk(5, &[777])]];
        let factory = CachedEvmFactory::new(chunks);

        // First create_evm → empty chunks → delegate to inner (live execution).
        let sender = address!("0x1000000000000000000000000000000000000000");
        let mut db_first = InMemoryDB::default();
        db_first.insert_account_info(
            sender,
            AccountInfo {
                balance: U256::from(1_000_000_000_000_000_000u128),
                nonce: 0,
                ..Default::default()
            },
        );
        let mut evm_first = factory.create_evm(db_first, test_env_for_block(7));
        let r_first = evm_first
            .transact_raw(make_transfer(sender, Address::ZERO, U256::from(1), 0))
            .unwrap();
        // Live execution — non-empty state.
        assert!(!r_first.state.is_empty());

        // Second create_evm → pops the next slot → returns the pre-computed result.
        let mut db_second = InMemoryDB::default();
        insert_stub_expected_state(&mut db_second);
        let mut evm_second = factory.create_evm(db_second, test_env_for_block(5));
        let r_second = evm_second
            .transact_raw(make_transfer(sender, Address::ZERO, U256::from(1), 0))
            .unwrap();
        match r_second.result {
            ExecutionResult::Success { gas_used, .. } => assert_eq!(gas_used, 777),
            _ => panic!("expected pre-computed Success result"),
        }
    }

    #[test]
    fn trait_method_field_access_works() {
        let factory = CachedEvmFactory::new(Vec::new());
        let db = InMemoryDB::default();
        let evm = factory.create_evm(db, test_env_for_block(42));
        assert_eq!(evm.block().number, U256::from(42));
        assert_eq!(evm.chain_id(), 1);
        let _ = evm.db();
    }

    #[test]
    fn take_next_chunks_pops_in_order() {
        // No EVM is built here — block_number is irrelevant, but pass a non-zero
        // value for consistency with other tests.
        let chunks = vec![vec![stub_chunk(1, &[123])]];
        let factory = CachedEvmFactory::new(chunks);

        // First call drains the single block's chunks.
        let drained = factory.take_next_chunks();
        assert_eq!(drained.len(), 1);
        // Second call returns empty.
        assert!(factory.take_next_chunks().is_empty());
    }

    #[test]
    fn exhausted_chunks_delegate_to_inner() {
        // Chunk has only one result.
        let chunks = vec![vec![stub_chunk(1, &[999])]];
        let factory = CachedEvmFactory::new(chunks);

        let sender = address!("0x1000000000000000000000000000000000000000");
        let mut db = InMemoryDB::default();
        insert_stub_expected_state(&mut db);
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
    }

    #[test]
    fn partial_execution_witness_rkyv_round_trip() {
        let addr = address!("0x1111111111111111111111111111111111111111");
        let code = Bytecode::new_raw(Bytes::from_static(&[0x60, 0x00, 0x60, 0x00]));
        let code_hash = code.hash_slow();
        let mut cache = CacheState::default();
        cache
            .accounts
            .insert(addr, CacheAccount::new_loaded_not_existing());
        cache.contracts.insert(code_hash, code.clone());

        let block_env = BlockEnv {
            number: U256::from(123),
            beneficiary: address!("0x2222222222222222222222222222222222222222"),
            timestamp: U256::from(1_700_000_000u64),
            gas_limit: 30_000_000,
            basefee: 7,
            difficulty: U256::ZERO,
            prevrandao: Some(B256::repeat_byte(0xAA)),
            ..Default::default()
        };

        let op_block_ctx = OpBlockExecutionCtx {
            parent_hash: B256::repeat_byte(0xBB),
            parent_beacon_block_root: Some(B256::repeat_byte(0xCC)),
            extra_data: Bytes::from_static(&[0xDE, 0xAD, 0xBE, 0xEF]),
        };

        let witness = PartialExecutionWitness {
            transactions: vec![vec![0x01, 0x02, 0x03], Vec::new(), vec![0xff; 32]],
            cache,
            block_env,
            op_block_ctx,
        };

        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&witness).unwrap();
        let recoded =
            rkyv::from_bytes::<PartialExecutionWitness, rkyv::rancor::Error>(&bytes).unwrap();

        // transactions: Vec<Vec<u8>> — direct equality.
        assert_eq!(witness.transactions, recoded.transactions);

        // BlockEnv derives PartialEq.
        assert_eq!(witness.block_env, recoded.block_env);

        // OpBlockExecutionCtx does not derive PartialEq — compare its three fields,
        // matching the pattern used by `op_block_execution_ctx_round_trip` in
        // `rkyv/evm.rs`.
        assert_eq!(
            witness.op_block_ctx.parent_hash,
            recoded.op_block_ctx.parent_hash
        );
        assert_eq!(
            witness.op_block_ctx.parent_beacon_block_root,
            recoded.op_block_ctx.parent_beacon_block_root
        );
        assert_eq!(
            witness.op_block_ctx.extra_data,
            recoded.op_block_ctx.extra_data
        );

        // Cache: verify the inserted entries survived and shape-level fields match.
        assert_eq!(recoded.cache.accounts.len(), 1);
        assert!(recoded.cache.accounts.contains_key(&addr));
        assert_eq!(recoded.cache.contracts.len(), 1);
        assert_eq!(
            recoded
                .cache
                .contracts
                .get(&code_hash)
                .map(|b| b.original_bytes()),
            Some(code.original_bytes())
        );
        assert_eq!(witness.cache.has_state_clear, recoded.cache.has_state_clear);
    }

    #[test]
    fn partial_account_revm_round_trip() {
        use crate::evm::partial::PartialAccount;
        use alloy_evm::revm::primitives::HashMap;
        use alloy_evm::revm::state::{AccountInfo, AccountStatus, EvmStorageSlot};

        let info = AccountInfo {
            nonce: 7,
            balance: U256::from(123_456u64),
            code_hash: B256::repeat_byte(0xAB),
            account_id: None,
            code: None,
        };
        let original_info = AccountInfo {
            nonce: 5,
            balance: U256::from(100_000u64),
            code_hash: B256::repeat_byte(0xCD),
            account_id: None,
            code: None,
        };

        // Insert in reverse-sorted order to exercise the sort invariant.
        let mut storage: HashMap<U256, EvmStorageSlot> = Default::default();
        storage.insert(
            U256::from(42),
            EvmStorageSlot::new_changed(U256::ZERO, U256::from(99), 0),
        );
        storage.insert(
            U256::from(7),
            EvmStorageSlot::new_changed(U256::from(1), U256::from(2), 0),
        );
        storage.insert(
            U256::from(13),
            EvmStorageSlot::new_changed(U256::from(3), U256::from(4), 0),
        );

        let account = Account {
            info,
            original_info: Box::new(original_info),
            transaction_id: 99,
            storage,
            status: AccountStatus::Touched | AccountStatus::Created,
        };

        // Capture an independent reference for set-equality comparison after
        // round-trip.
        let original_storage_set: std::collections::BTreeSet<(U256, U256, U256)> = account
            .storage
            .iter()
            .map(|(k, v)| (*k, v.original_value, v.present_value))
            .collect();

        let partial = PartialAccount::from(account);

        // Sort invariant on the Vec.
        assert!(
            partial.storage.windows(2).all(|w| w[0].slot < w[1].slot),
            "PartialAccount::from(Account) must produce storage sorted by slot",
        );
        assert_eq!(partial.storage.len(), 3);

        // Round-trip back to revm.
        let rebuilt: Account = partial.into();
        assert_eq!(rebuilt.info.nonce, 7);
        assert_eq!(rebuilt.original_info.nonce, 5);
        assert_eq!(rebuilt.transaction_id, 99);
        assert_eq!(rebuilt.storage.len(), 3);
        let rebuilt_set: std::collections::BTreeSet<(U256, U256, U256)> = rebuilt
            .storage
            .iter()
            .map(|(k, v)| (*k, v.original_value, v.present_value))
            .collect();
        assert_eq!(original_storage_set, rebuilt_set);
    }

    #[test]
    fn partial_result_and_state_rkyv_round_trip() {
        use crate::evm::partial::PartialResultAndState;
        use alloy_evm::revm::primitives::HashMap;
        use alloy_evm::revm::state::{AccountInfo, AccountStatus, EvmStorageSlot};

        let make_account = |seed: u8| {
            let mut storage: HashMap<U256, EvmStorageSlot> = Default::default();
            storage.insert(
                U256::from(2u64),
                EvmStorageSlot::new_changed(U256::ZERO, U256::from(seed as u64 + 100), 0),
            );
            storage.insert(
                U256::from(1u64),
                EvmStorageSlot::new_changed(U256::ZERO, U256::from(seed as u64 + 200), 0),
            );
            Account {
                info: AccountInfo {
                    nonce: seed as u64,
                    balance: U256::from(seed as u64 * 1000),
                    code_hash: B256::repeat_byte(seed),
                    account_id: None,
                    code: None,
                },
                original_info: Box::new(AccountInfo {
                    nonce: 0,
                    balance: U256::ZERO,
                    code_hash: B256::ZERO,
                    account_id: None,
                    code: None,
                }),
                transaction_id: seed as usize,
                storage,
                status: AccountStatus::Touched,
            }
        };

        let mut state: alloy_evm::revm::primitives::HashMap<Address, Account> = Default::default();
        // Insert out of address order.
        state.insert(
            address!("0xCCCC000000000000000000000000000000000000"),
            make_account(0xCC),
        );
        state.insert(
            address!("0xAAAA000000000000000000000000000000000000"),
            make_account(0xAA),
        );
        state.insert(
            address!("0xBBBB000000000000000000000000000000000000"),
            make_account(0xBB),
        );

        let revm_ras = ResultAndState {
            result: ExecutionResult::Success {
                reason: SuccessReason::Return,
                gas_used: 50_000,
                gas_refunded: 0,
                logs: vec![],
                output: Output::Call(Bytes::new()),
            },
            state,
        };

        let partial = PartialResultAndState::from(revm_ras);

        // Sort invariants.
        assert!(
            partial
                .state
                .windows(2)
                .all(|w| w[0].address < w[1].address),
            "state must be sorted by address",
        );
        for entry in &partial.state {
            assert!(
                entry
                    .account
                    .storage
                    .windows(2)
                    .all(|w| w[0].slot < w[1].slot),
                "per-account storage must be sorted by slot",
            );
        }

        // rkyv round-trip.
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&partial).unwrap();
        let recoded: PartialResultAndState =
            rkyv::from_bytes::<PartialResultAndState, rkyv::rancor::Error>(&bytes).unwrap();

        // Sort invariants survive the round-trip.
        assert!(recoded
            .state
            .windows(2)
            .all(|w| w[0].address < w[1].address));
        for entry in &recoded.state {
            assert!(entry
                .account
                .storage
                .windows(2)
                .all(|w| w[0].slot < w[1].slot));
        }

        // Field equality. ExecutionResult success-variant fields:
        match (&partial.result, &recoded.result) {
            (
                ExecutionResult::Success { gas_used: a, .. },
                ExecutionResult::Success { gas_used: b, .. },
            ) => assert_eq!(a, b),
            _ => panic!("ExecutionResult variant changed across rkyv round-trip"),
        }
        assert_eq!(partial.state.len(), recoded.state.len());
        for (a_entry, b_entry) in partial.state.iter().zip(recoded.state.iter()) {
            assert_eq!(a_entry.address, b_entry.address);
            let a_acc = &a_entry.account;
            let b_acc = &b_entry.account;
            assert_eq!(a_acc.info.nonce, b_acc.info.nonce);
            assert_eq!(a_acc.info.balance, b_acc.info.balance);
            assert_eq!(a_acc.info.code_hash, b_acc.info.code_hash);
            assert_eq!(a_acc.original_info.nonce, b_acc.original_info.nonce);
            assert_eq!(a_acc.status.bits(), b_acc.status.bits());
            assert_eq!(a_acc.storage.len(), b_acc.storage.len());
            for (sa, sb) in a_acc.storage.iter().zip(b_acc.storage.iter()) {
                assert_eq!(sa.slot, sb.slot);
                assert_eq!(sa.slot_value.original_value, sb.slot_value.original_value);
                assert_eq!(sa.slot_value.present_value, sb.slot_value.present_value);
            }
        }

        // Round-trip to revm: HashMap rebuild must preserve set-equality of
        // (addr, slot, original, present) tuples.
        let rebuilt: ResultAndState<OpHaltReason> = recoded.into();
        assert_eq!(rebuilt.state.len(), partial.state.len());
        for entry in &partial.state {
            let rebuilt_acc = rebuilt
                .state
                .get(&entry.address)
                .expect("rebuilt state missing address");
            assert_eq!(rebuilt_acc.storage.len(), entry.account.storage.len());
            for slot_entry in &entry.account.storage {
                let rebuilt_v = rebuilt_acc
                    .storage
                    .get(&slot_entry.slot)
                    .expect("rebuilt account missing slot");
                assert_eq!(
                    rebuilt_v.original_value,
                    slot_entry.slot_value.original_value
                );
                assert_eq!(rebuilt_v.present_value, slot_entry.slot_value.present_value);
            }
        }
    }

    #[test]
    fn witness_cache_round_trips_expected_state() {
        use crate::evm::expected::capture_required_expected_state;
        use crate::precondition::evm::hash_expected_state;
        use alloy_evm::revm::database::State;

        // The host derives `expected_state` via
        // `capture_required_expected_state` during preflight. This test
        // proves that re-running the same routine against a State backed
        // by the witness's cache reproduces the same snapshot — so the
        // chunk's `pe_trace` will match what the host hashed in.
        let mut seed_db = InMemoryDB::default();
        insert_stub_expected_state(&mut seed_db);
        let original = capture_required_expected_state(&mut seed_db);

        let mut chunk = stub_chunk(1, &[42]);
        chunk.expected_state = original.clone();
        let witness = PartialExecutionWitness::new(chunk, vec![]);

        let mut state = State::builder()
            .with_database(InMemoryDB::default())
            .with_cached_prestate(witness.cache)
            .build();
        let derived = capture_required_expected_state(&mut state);

        assert_eq!(
            hash_expected_state(&original),
            hash_expected_state(&derived)
        );
    }

    #[test]
    fn apply_result_to_expected_state_does_not_add_new_slots() {
        use crate::evm::expected::apply_result_to_expected_state;

        // A tx result that writes to a slot NOT already in
        // `expected_state` must leave `expected_state` unchanged in slot
        // count — adding new slots would inflate the splitter's
        // per-chunk snapshot beyond what the spec-bounded re-derive
        // produces, breaking the pe_trace round-trip.
        let mut expected = stub_expected_state();
        let initial_slot_count = expected[0].account.storage.len();
        let extra_slot = U256::from(0xDEADu64);
        let result =
            stub_result_with_l1_block_slot_change(10, extra_slot, U256::ZERO, U256::from(99));

        apply_result_to_expected_state(&mut expected, &result);

        assert_eq!(expected[0].account.storage.len(), initial_slot_count);
        assert!(expected[0]
            .account
            .storage
            .iter()
            .all(|s| s.slot != extra_slot));
    }
}

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

/// EVM factory that replays cached per-transaction results instead of re-executing.
pub mod cached;
/// The out-of-transaction state read by OP execution logic (the L1Block predeploy).
pub mod expected;
/// Chunk data structures for proving transaction subsequences within a block.
pub mod partial;
/// Witness data for running a partial block execution in-guest.
pub mod witness;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use crate::boot::L1_HEAD_TXN_ONLY_SENTINEL;
    use crate::evm::cached::CachedEvmFactory;
    use crate::evm::expected::{
        apply_result_to_expected_state, canonicalize_expected_state,
        capture_required_expected_state, ExpectedAccount, ExpectedStateEntry, ExpectedStorageEntry,
        EXPECTED_STATE_ADDRESSES, EXPECTED_STORAGE_SLOTS,
    };
    use crate::evm::partial::{
        PartialAccount, PartialExecution, PartialResultAndState, PartialStateEntry,
        PartialStorageEntry, TransactionResultCollector,
    };
    use crate::evm::witness::{cache_results, PartialExecutionWitness};
    use alloy_evm::revm::context::CfgEnv;
    use alloy_evm::revm::context::{BlockEnv, TxEnv};
    use alloy_evm::revm::context_interface::result::ResultAndState;
    use alloy_evm::revm::context_interface::result::{
        ExecutionResult, Output, ResultGas, SuccessReason,
    };
    use alloy_evm::revm::database::in_memory_db::InMemoryDB;
    use alloy_evm::revm::database::states::CacheAccount;
    use alloy_evm::revm::database::CacheState;
    use alloy_evm::revm::primitives::HashMap as RevmHashMap;
    use alloy_evm::revm::state::Account;
    use alloy_evm::revm::state::AccountInfo;
    use alloy_evm::revm::state::AccountStatus;
    use alloy_evm::revm::state::Bytecode;
    use alloy_evm::revm::state::EvmStorageSlot;
    use alloy_evm::revm::state::{EvmState, EvmStorage};
    use alloy_evm::revm::DatabaseCommit;
    use alloy_evm::{Evm, EvmEnv, EvmFactory};
    use alloy_op_evm::block::{OpBlockExecutionCtx, PostExecMode};
    use alloy_op_evm::OpTx;
    use alloy_primitives::Address;
    use alloy_primitives::{address, TxKind, U256};
    use alloy_primitives::{Bytes, B256};
    use kona_proof::BootInfo;
    use op_revm::{constants::L1_BLOCK_CONTRACT, OpHaltReason, OpSpecId, OpTransaction};
    use risc0_zkvm::sha::{Impl as SHA2, Sha256};
    use std::sync::{Arc, Mutex};

    // ============================================================
    // Helpers
    // ============================================================

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

    fn make_transfer(caller: Address, to: Address, value: U256, nonce: u64) -> OpTx {
        OpTx(OpTransaction {
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
        })
    }

    /// `make_transfer` with a custom envelope (or `None` to drive the
    /// missing-envelope panic invariant).
    fn make_transfer_with_envelope(envelope: Option<Bytes>, caller: Address) -> OpTx {
        OpTx(OpTransaction {
            base: TxEnv {
                caller,
                kind: TxKind::Call(Address::ZERO),
                value: U256::ZERO,
                gas_limit: 21000,
                gas_price: 1,
                nonce: 0,
                ..Default::default()
            },
            enveloped_tx: envelope,
            ..Default::default()
        })
    }

    fn stub_result_and_state(gas_used: u64) -> ResultAndState<OpHaltReason> {
        ResultAndState {
            result: ExecutionResult::Success {
                reason: SuccessReason::Return,
                gas: ResultGas::default().with_total_gas_spent(gas_used),
                logs: vec![],
                output: Output::Call(Bytes::new()),
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
        let mut storage: EvmStorage = Default::default();
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
                gas: ResultGas::default().with_total_gas_spent(gas_used),
                logs: vec![],
                output: Output::Call(Bytes::new()),
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
        let default_tx_hash = B256::from_slice(SHA2::hash_bytes(&[0x00u8]).as_bytes());
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
            op_block_ctx: OpBlockExecutionCtx::default(),
        }
    }

    /// Build a chunk whose single result carries one explicit state entry,
    /// for exercising per-account / per-slot cached-path invariants.
    fn stub_chunk_with_state(
        block_number: u64,
        gas_used: u64,
        addr: Address,
        account: Account,
    ) -> PartialExecution {
        let mut state = RevmHashMap::default();
        state.insert(addr, account);
        let block_env = BlockEnv {
            number: U256::from(block_number),
            ..Default::default()
        };
        PartialExecution {
            tx_hashes: vec![B256::from_slice(SHA2::hash_bytes(&[0x00u8]).as_bytes())],
            results: vec![PartialResultAndState::from(ResultAndState {
                result: ExecutionResult::Success {
                    reason: SuccessReason::Return,
                    gas: ResultGas::default().with_total_gas_spent(gas_used),
                    logs: vec![],
                    output: Output::Call(Bytes::new()),
                },
                state,
            })],
            expected_state: stub_expected_state(),
            block_env,
            op_block_ctx: OpBlockExecutionCtx::default(),
        }
    }

    fn funded_db() -> (InMemoryDB, Address, Address) {
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
        (db, sender, recipient)
    }

    fn unwrap_success_gas(r: &ExecutionResult<OpHaltReason>) -> u64 {
        match r {
            ExecutionResult::Success { gas, .. } => gas.total_gas_spent(),
            _ => panic!("expected ExecutionResult::Success, got {r:?}"),
        }
    }

    // ============================================================
    // CachedEvm: serve-from-cache happy paths (consolidated)
    // ============================================================

    struct ServeOrderCase {
        label: &'static str,
        marker_groups: Vec<Vec<u64>>,
        expected: Vec<u64>,
        last_tx_delegates: bool,
    }

    /// Walks three serving topologies in one body — single chunk N=3,
    /// multi-chunk in one block, and chunk-then-exhaust-then-delegate.
    /// Replaces three previously-separate sparse tests.
    #[test]
    fn cached_chunks_served_in_order_matrix() {
        let scenarios = vec![
            ServeOrderCase {
                label: "single chunk N=3",
                marker_groups: vec![vec![100_001, 100_002, 100_003]],
                expected: vec![100_001, 100_002, 100_003],
                last_tx_delegates: false,
            },
            ServeOrderCase {
                label: "two chunks N=2+N=2",
                marker_groups: vec![vec![10, 20], vec![30, 40]],
                expected: vec![10, 20, 30, 40],
                last_tx_delegates: false,
            },
            ServeOrderCase {
                label: "exhaust then delegate",
                marker_groups: vec![vec![999]],
                expected: vec![999],
                last_tx_delegates: true,
            },
        ];

        for case in scenarios {
            let chunks = vec![case
                .marker_groups
                .iter()
                .map(|m| stub_chunk(1, m))
                .collect::<Vec<_>>()];
            let factory = CachedEvmFactory::new(chunks);
            let (db, sender, recipient) = funded_db();
            let mut evm = factory.create_evm(db, test_env_for_block(1));

            for (i, gas) in case.expected.iter().enumerate() {
                let r = evm
                    .transact_raw(make_transfer(sender, recipient, U256::from(1), 0))
                    .unwrap();
                assert_eq!(
                    unwrap_success_gas(&r.result),
                    *gas,
                    "{}: tx {}",
                    case.label,
                    i
                );
                assert!(
                    r.state.is_empty(),
                    "{}: cached results carry empty state",
                    case.label
                );
            }

            if case.last_tx_delegates {
                // One additional tx after exhaustion delegates → live state
                // diff. nonce is still 0 because cached results carry empty
                // state (the DB was never mutated by the pre-computed tx).
                let r = evm
                    .transact_raw(make_transfer(sender, recipient, U256::from(1), 0))
                    .unwrap();
                assert!(
                    !r.state.is_empty(),
                    "{}: delegated tx must populate state",
                    case.label
                );
            }
        }
    }

    /// Cache-miss fallback paths: empty chunks for the block, and non-empty
    /// chunks where the incoming tx hash doesn't match the chunk's head.
    #[test]
    fn cache_miss_fallback_matrix() {
        // (A) Empty chunks → all txs delegate.
        {
            let factory = CachedEvmFactory::new(Vec::new());
            let (db, sender, recipient) = funded_db();
            let mut evm = factory.create_evm(db, test_env_for_block(1));
            let r = evm
                .transact_raw(make_transfer(sender, recipient, U256::from(1000), 0))
                .unwrap();
            assert!(
                r.state.contains_key(&sender) && r.state.contains_key(&recipient),
                "(A) live execution must touch both accounts",
            );
        }

        // (B) Non-empty chunks but tx envelope doesn't match the chunk's
        // tx_hash → falls through to live execution. The chunk's results
        // remain on the cache (none consumed).
        {
            let factory = CachedEvmFactory::new(vec![vec![stub_chunk(1, &[42])]]);
            let (db, sender, recipient) = funded_db();
            let mut evm = factory.create_evm(db, test_env_for_block(1));
            // Use a non-default envelope so sha256(envelope) != default_tx_hash.
            let mut tx = make_transfer(sender, recipient, U256::from(1000), 0);
            tx.enveloped_tx = Some(Bytes::from_static(&[0xFF, 0xEE]));
            let r = evm.transact_raw(tx).unwrap();
            assert!(
                !r.state.is_empty(),
                "(B) non-matching envelope must delegate"
            );
        }
    }

    /// `expected_state` validation runs once per chunk on first cached
    /// dispatch — subsequent cached results bypass the check, which is
    /// load-bearing because the running DB will have been mutated by
    /// the first tx's commit.
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

        // Without the expected_state_verified guard, this second tx would
        // see slot[0]=1 in the DB but expected_state still claims 0 → panic.
        let second = evm
            .transact_raw(make_transfer(sender, Address::ZERO, U256::ZERO, 0))
            .unwrap();
        assert_eq!(unwrap_success_gas(&second.result), 20);
    }

    // ============================================================
    // CachedEvm: panic invariants on the cached path
    //
    // Each test must have all checks before its target invariant
    // satisfied — see the file-level note about the cached-path
    // assertion order.
    // ============================================================

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
        // Diverge one slot from what the chunk's expected_state claims.
        db.insert_account_storage(L1_BLOCK_CONTRACT, EXPECTED_STORAGE_SLOTS[0], U256::from(1))
            .unwrap();
        let mut evm = factory.create_evm(db, test_env_for_block(1));
        let sender = address!("0x1000000000000000000000000000000000000000");
        let _ = evm
            .transact_raw(make_transfer(sender, Address::ZERO, U256::ZERO, 0))
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "BlockEnv mismatch")]
    fn cached_chunk_block_env_mismatch_panics() {
        // chunk says block 99; EVM is built for block 1.
        let factory = CachedEvmFactory::new(vec![vec![stub_chunk(99, &[42])]]);
        let mut db = InMemoryDB::default();
        insert_stub_expected_state(&mut db);
        let mut evm = factory.create_evm(db, test_env_for_block(1));
        let sender = address!("0x1000000000000000000000000000000000000000");
        let _ = evm
            .transact_raw(make_transfer(sender, Address::ZERO, U256::ZERO, 0))
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "storage prestate mismatch")]
    fn cached_chunk_storage_prestate_mismatch_panics() {
        // Chunk's expected_state matches DB (all slots = 0), but the
        // result claims a per-slot original_value of 99 — divergent.
        let mut storage: EvmStorage = Default::default();
        storage.insert(
            EXPECTED_STORAGE_SLOTS[0],
            EvmStorageSlot::new_changed(U256::from(99), U256::from(100), 0),
        );
        let account = Account {
            info: AccountInfo::default(),
            original_info: Box::new(AccountInfo::default()),
            transaction_id: 0,
            storage,
            status: AccountStatus::Touched,
        };
        let chunk = stub_chunk_with_state(1, 42, L1_BLOCK_CONTRACT, account);
        let factory = CachedEvmFactory::new(vec![vec![chunk]]);
        let mut db = InMemoryDB::default();
        insert_stub_expected_state(&mut db);
        let mut evm = factory.create_evm(db, test_env_for_block(1));
        let sender = address!("0x1000000000000000000000000000000000000000");
        let _ = evm
            .transact_raw(make_transfer(sender, Address::ZERO, U256::ZERO, 0))
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "account prestate mismatch")]
    fn cached_chunk_account_prestate_mismatch_panics() {
        // DB has L1_BLOCK_CONTRACT with default AccountInfo; chunk's
        // result claims original_info.nonce = 99 — divergent.
        let account = Account {
            info: AccountInfo::default(),
            original_info: Box::new(AccountInfo {
                nonce: 99,
                ..Default::default()
            }),
            transaction_id: 0,
            storage: Default::default(),
            status: AccountStatus::Touched,
        };
        let chunk = stub_chunk_with_state(1, 42, L1_BLOCK_CONTRACT, account);
        let factory = CachedEvmFactory::new(vec![vec![chunk]]);
        let mut db = InMemoryDB::default();
        insert_stub_expected_state(&mut db);
        let mut evm = factory.create_evm(db, test_env_for_block(1));
        let sender = address!("0x1000000000000000000000000000000000000000");
        let _ = evm
            .transact_raw(make_transfer(sender, Address::ZERO, U256::ZERO, 0))
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "Unexpected AccountStatus for non-existing account")]
    fn cached_chunk_unexpected_account_status_panics() {
        // Address NOT in DB; result claims status `Touched` (neither
        // Created nor LoadedAsNotExisting) → invariant violation.
        let new_addr = address!("0x9999999999999999999999999999999999999999");
        let account = Account {
            info: AccountInfo::default(),
            original_info: Box::new(AccountInfo::default()),
            transaction_id: 0,
            storage: Default::default(),
            status: AccountStatus::Touched,
        };
        let chunk = stub_chunk_with_state(1, 42, new_addr, account);
        let factory = CachedEvmFactory::new(vec![vec![chunk]]);
        let mut db = InMemoryDB::default();
        insert_stub_expected_state(&mut db);
        let mut evm = factory.create_evm(db, test_env_for_block(1));
        let sender = address!("0x1000000000000000000000000000000000000000");
        let _ = evm
            .transact_raw(make_transfer(sender, Address::ZERO, U256::ZERO, 0))
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "OpTransaction.enveloped_tx must be populated")]
    fn cached_chunk_missing_enveloped_tx_panics() {
        let factory = CachedEvmFactory::new(vec![vec![stub_chunk(1, &[42])]]);
        let mut db = InMemoryDB::default();
        insert_stub_expected_state(&mut db);
        let mut evm = factory.create_evm(db, test_env_for_block(1));
        let sender = address!("0x1000000000000000000000000000000000000000");
        let tx = make_transfer_with_envelope(None, sender);
        let _ = evm.transact_raw(tx).unwrap();
    }

    // ============================================================
    // CachedEvm: trait method delegation matrix
    // ============================================================

    /// Walks every trait-method pass-through that the compiler can't
    /// otherwise prove is exercised: block, chain_id, components,
    /// components_mut, set_inspector_enabled, finish.
    #[test]
    fn evm_trait_delegation_matrix() {
        let factory = CachedEvmFactory::new(Vec::new());
        let db = InMemoryDB::default();
        let mut evm = factory.create_evm(db, test_env_for_block(42));

        // block() / chain_id()
        assert_eq!(evm.block().number, U256::from(42));
        assert_eq!(evm.chain_id(), 1);

        // db() / db_mut()
        let _ = evm.db();
        let _ = evm.db_mut();

        // set_inspector_enabled both states.
        evm.set_inspector_enabled(true);
        evm.set_inspector_enabled(false);

        // components() / components_mut() — accessor sanity.
        let _ = evm.components();
        let _ = evm.components_mut();

        // finish() consumes — must return the (DB, EvmEnv) pair.
        let (_db_returned, env_returned) = evm.finish();
        assert_eq!(env_returned.block_env.number, U256::from(42));
        assert_eq!(env_returned.cfg_env.chain_id, 1);
    }

    /// System calls bypass the chunk cache entirely — the chunk's first
    /// result must still be served on the next user-tx after the system
    /// call completes.
    #[test]
    fn system_calls_delegate_and_do_not_advance_tx_index() {
        let factory = CachedEvmFactory::new(vec![vec![stub_chunk(1, &[42])]]);
        let mut db = InMemoryDB::default();
        insert_stub_expected_state(&mut db);
        let mut evm = factory.create_evm(db, test_env_for_block(1));

        let caller = address!("0xdeaddeaddeaddeaddeaddeaddeaddeaddead0001");
        let contract = address!("0x000F3df6D732807Ef1319fB7B8bB8522d0Beac02");
        let _ = evm.transact_system_call(caller, contract, Bytes::new());

        let sender = address!("0x1000000000000000000000000000000000000000");
        let r = evm
            .transact_raw(make_transfer(sender, Address::ZERO, U256::ZERO, 0))
            .unwrap();
        assert_eq!(unwrap_success_gas(&r.result), 42);
    }

    // ============================================================
    // CachedEvmFactory
    // ============================================================

    /// Walks the factory's three slot-management invariants in one body:
    ///   - per-EVM creation drains in creation order
    ///   - first-EVM gets first slot's chunks (live execution if empty)
    ///   - `take_next_chunks` exhausts the queue
    #[test]
    fn factory_creation_order_and_drain() {
        // Slot 0: empty → first EVM falls through to live execution.
        // Slot 1: chunk for block 5 with marker 777.
        let factory = CachedEvmFactory::new(vec![vec![], vec![stub_chunk(5, &[777])]]);

        // First EVM (slot 0) — empty.
        let (db, sender, recipient) = funded_db();
        let mut evm_first = factory.create_evm(db, test_env_for_block(7));
        let r1 = evm_first
            .transact_raw(make_transfer(sender, recipient, U256::from(1), 0))
            .unwrap();
        assert!(!r1.state.is_empty());

        // Second EVM (slot 1) — gets the cached marker.
        let mut db = InMemoryDB::default();
        insert_stub_expected_state(&mut db);
        let mut evm_second = factory.create_evm(db, test_env_for_block(5));
        let r2 = evm_second
            .transact_raw(make_transfer(sender, Address::ZERO, U256::from(1), 0))
            .unwrap();
        assert_eq!(unwrap_success_gas(&r2.result), 777);

        // After both EVMs were built, take_next_chunks should be drained.
        assert!(factory.take_next_chunks().is_empty());

        // A fresh factory's drain semantics (separate from the EVM-creation
        // drain above): one slot pops once, then returns empty.
        let fresh = CachedEvmFactory::new(vec![vec![stub_chunk(1, &[123])]]);
        assert_eq!(fresh.take_next_chunks().len(), 1);
        assert!(fresh.take_next_chunks().is_empty());
    }

    /// Trace collection only fires on cache-miss execution paths. After
    /// running one live tx, the collector must hold one entry; cached
    /// dispatches don't accumulate. `take_all_block_traces` drains.
    #[test]
    fn factory_collects_block_traces_when_attached() {
        let collector: TransactionResultCollector = Arc::new(Mutex::new(Vec::new()));
        // Slot 0: one chunk with one marker (cached).
        // Slot 1: empty (forces live for the tx in the second EVM).
        let factory = CachedEvmFactory::new_with_traces(
            vec![vec![stub_chunk(1, &[42])], vec![]],
            Some(collector.clone()),
        );

        // EVM 1 — cached path, no trace pushed.
        let mut db = InMemoryDB::default();
        insert_stub_expected_state(&mut db);
        let mut evm1 = factory.create_evm(db, test_env_for_block(1));
        let _ = evm1
            .transact_raw(make_transfer(
                address!("0x1000000000000000000000000000000000000000"),
                Address::ZERO,
                U256::ZERO,
                0,
            ))
            .unwrap();

        // EVM 2 — live path, one trace pushed into the second slot.
        let (db, sender, recipient) = funded_db();
        let mut evm2 = factory.create_evm(db, test_env_for_block(1));
        let _ = evm2
            .transact_raw(make_transfer(sender, recipient, U256::from(1), 0))
            .unwrap();

        // Drain & assert. Two slots were pushed (one per create_evm).
        let drained = factory.take_all_block_traces();
        assert_eq!(drained.len(), 2, "one trace slot per create_evm");
        assert_eq!(drained[0].len(), 0, "cached path doesn't push");
        assert_eq!(drained[1].len(), 1, "live path pushes one trace");

        // Second drain returns nothing — the collector was moved out.
        assert!(factory.take_all_block_traces().is_empty());

        // Factory without attached collector returns empty drain.
        let no_traces = CachedEvmFactory::new(vec![]);
        assert!(no_traces.take_all_block_traces().is_empty());
    }

    /// `create_evm_with_inspector` returns a working CachedEvm — exercises
    /// the inspector-variant constructor that `create_evm` doesn't reach.
    #[test]
    fn factory_create_evm_with_inspector_returns_cached_evm() {
        use alloy_evm::revm::inspector::NoOpInspector;
        let factory = CachedEvmFactory::new(vec![vec![stub_chunk(1, &[7])]]);
        let mut db = InMemoryDB::default();
        insert_stub_expected_state(&mut db);
        let mut evm =
            factory.create_evm_with_inspector(db, test_env_for_block(1), NoOpInspector {});
        let sender = address!("0x1000000000000000000000000000000000000000");
        let r = evm
            .transact_raw(make_transfer(sender, Address::ZERO, U256::ZERO, 0))
            .unwrap();
        assert_eq!(unwrap_success_gas(&r.result), 7);
    }

    // ============================================================
    // expected.rs
    // ============================================================

    /// `capture_required_expected_state` returns one entry per address
    /// in `EXPECTED_STATE_ADDRESSES` and one slot per
    /// `EXPECTED_STORAGE_SLOTS`. `canonicalize_expected_state` sorts
    /// both axes.
    #[test]
    fn capture_and_canonicalize_expected_state() {
        let mut db = InMemoryDB::default();
        insert_stub_expected_state(&mut db);
        let captured = capture_required_expected_state(&mut db);
        assert_eq!(captured.len(), EXPECTED_STATE_ADDRESSES.len());
        for entry in &captured {
            assert_eq!(entry.account.storage.len(), EXPECTED_STORAGE_SLOTS.len());
            assert!(entry.account.exists);
        }

        // Capture against an empty DB → exists=false.
        let mut empty_db = InMemoryDB::default();
        let captured_empty = capture_required_expected_state(&mut empty_db);
        assert_eq!(captured_empty.len(), EXPECTED_STATE_ADDRESSES.len());
        assert!(!captured_empty[0].account.exists);

        // canonicalize sorts addresses (single address case is trivially
        // sorted; build a multi-entry input to actually exercise the sort).
        let unsorted = vec![
            ExpectedStateEntry {
                address: address!("0xCCCC000000000000000000000000000000000000"),
                account: ExpectedAccount {
                    exists: true,
                    info: AccountInfo::default(),
                    storage: vec![
                        ExpectedStorageEntry {
                            slot: U256::from(5),
                            value: U256::ZERO,
                        },
                        ExpectedStorageEntry {
                            slot: U256::from(1),
                            value: U256::ZERO,
                        },
                    ],
                },
            },
            ExpectedStateEntry {
                address: address!("0xAAAA000000000000000000000000000000000000"),
                account: ExpectedAccount {
                    exists: true,
                    info: AccountInfo::default(),
                    storage: vec![],
                },
            },
        ];
        let canon = canonicalize_expected_state(unsorted);
        assert!(canon.windows(2).all(|w| w[0].address < w[1].address));
        // Per-account storage sort: check the entry that actually has
        // multi-slot storage. canon[0] (AAAA) has empty storage, so
        // `windows(2).all(...)` would be vacuously true and never invoke
        // the slot-compare closure.
        let cccc_entry = canon
            .iter()
            .find(|e| e.address == address!("0xCCCC000000000000000000000000000000000000"))
            .expect("CCCC entry must exist after canonicalize");
        assert!(cccc_entry
            .account
            .storage
            .windows(2)
            .all(|w| w[0].slot < w[1].slot));
        assert_eq!(cccc_entry.account.storage[0].slot, U256::from(1));
        assert_eq!(cccc_entry.account.storage[1].slot, U256::from(5));
    }

    /// All five shaping rules of `apply_result_to_expected_state` in one
    /// matrix body:
    ///   - existing slot value gets overwritten with present_value
    ///   - info gets overwritten from result
    ///   - LoadedAsNotExisting flips exists=false
    ///   - extra slot from result is silently skipped
    ///   - irrelevant address is silently skipped (never inserted)
    #[test]
    fn apply_result_to_expected_state_matrix() {
        // (1) Slot value update.
        {
            let mut expected = stub_expected_state();
            let slot = EXPECTED_STORAGE_SLOTS[0];
            let result =
                stub_result_with_l1_block_slot_change(10, slot, U256::ZERO, U256::from(0xAB));
            apply_result_to_expected_state(&mut expected, &result);
            let updated = expected[0]
                .account
                .storage
                .iter()
                .find(|s| s.slot == slot)
                .expect("slot must remain");
            assert_eq!(updated.value, U256::from(0xAB));
        }

        // (2) Info & exists-status update.
        {
            let mut expected = stub_expected_state();
            let new_info = AccountInfo {
                nonce: 5,
                balance: U256::from(7777),
                ..Default::default()
            };
            let mut state_map = RevmHashMap::default();
            state_map.insert(
                L1_BLOCK_CONTRACT,
                Account {
                    info: new_info.clone(),
                    original_info: Box::new(AccountInfo::default()),
                    transaction_id: 0,
                    storage: Default::default(),
                    status: AccountStatus::Touched,
                },
            );
            let result = PartialResultAndState::from(ResultAndState::<OpHaltReason> {
                result: ExecutionResult::Success {
                    reason: SuccessReason::Return,
                    gas: ResultGas::default().with_total_gas_spent(1),
                    logs: vec![],
                    output: Output::Call(Bytes::new()),
                },
                state: state_map,
            });
            apply_result_to_expected_state(&mut expected, &result);
            assert_eq!(expected[0].account.info.nonce, 5);
            assert!(expected[0].account.exists);
        }

        // (3) LoadedAsNotExisting → exists=false.
        {
            let mut expected = stub_expected_state();
            let mut state_map = RevmHashMap::default();
            state_map.insert(
                L1_BLOCK_CONTRACT,
                Account {
                    info: AccountInfo::default(),
                    original_info: Box::new(AccountInfo::default()),
                    transaction_id: 0,
                    storage: Default::default(),
                    status: AccountStatus::LoadedAsNotExisting,
                },
            );
            let result = PartialResultAndState::from(ResultAndState::<OpHaltReason> {
                result: ExecutionResult::Success {
                    reason: SuccessReason::Return,
                    gas: ResultGas::default().with_total_gas_spent(1),
                    logs: vec![],
                    output: Output::Call(Bytes::new()),
                },
                state: state_map,
            });
            apply_result_to_expected_state(&mut expected, &result);
            assert!(!expected[0].account.exists);
        }

        // (4) Extra slot in result is dropped, slot count is invariant.
        {
            let mut expected = stub_expected_state();
            let initial_count = expected[0].account.storage.len();
            let result = stub_result_with_l1_block_slot_change(
                10,
                U256::from(0xDEADu64),
                U256::ZERO,
                U256::from(99),
            );
            apply_result_to_expected_state(&mut expected, &result);
            assert_eq!(expected[0].account.storage.len(), initial_count);
        }

        // (5) Irrelevant address is silently skipped (never inserted).
        {
            let mut expected = stub_expected_state();
            let initial_addr_count = expected.len();
            let irrelevant = address!("0x0000000000000000000000000000000000000777");
            let mut state_map = RevmHashMap::default();
            state_map.insert(
                irrelevant,
                Account {
                    info: AccountInfo::default(),
                    original_info: Box::new(AccountInfo::default()),
                    transaction_id: 0,
                    storage: Default::default(),
                    status: AccountStatus::Touched,
                },
            );
            let result = PartialResultAndState::from(ResultAndState::<OpHaltReason> {
                result: ExecutionResult::Success {
                    reason: SuccessReason::Return,
                    gas: ResultGas::default().with_total_gas_spent(1),
                    logs: vec![],
                    output: Output::Call(Bytes::new()),
                },
                state: state_map,
            });
            apply_result_to_expected_state(&mut expected, &result);
            assert_eq!(expected.len(), initial_addr_count);
            assert!(expected.iter().all(|e| e.address != irrelevant));
        }

        // (6) L1Block in result but missing from expected_state — the
        // `let Some(expected_entry) = expected_by_address.get_mut(...)`
        // miss path must `continue` without inserting.
        {
            let mut expected: Vec<ExpectedStateEntry> = vec![];
            let result = stub_result_with_l1_block_slot_change(
                10,
                EXPECTED_STORAGE_SLOTS[0],
                U256::ZERO,
                U256::from(0xAB),
            );
            apply_result_to_expected_state(&mut expected, &result);
            assert!(expected.is_empty(), "must not insert new entries");
        }
    }

    // ============================================================
    // partial.rs
    // ============================================================

    /// Round-trips an `Account` through `PartialAccount` and back, asserting
    /// (a) sort invariant on the Vec, (b) field equality after rebuild.
    #[test]
    fn partial_account_revm_round_trip() {
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
        // Insert in reverse-sorted order to exercise the sort.
        let mut storage: EvmStorage = Default::default();
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
        let original_storage_set: std::collections::BTreeSet<(U256, U256, U256)> = account
            .storage
            .iter()
            .map(|(k, v)| (*k, v.original_value, v.present_value))
            .collect();

        let partial = PartialAccount::from(account);
        assert!(
            partial.storage.windows(2).all(|w| w[0].slot < w[1].slot),
            "PartialAccount::from must produce sorted storage",
        );
        assert_eq!(partial.storage.len(), 3);

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

    /// Round-trips a `ResultAndState` through `PartialResultAndState` then
    /// back via rkyv, asserting:
    ///   - sort invariants survive each hop
    ///   - per-field equality after rebuild
    ///   - HashMap rebuild preserves set-equality of (addr, slot, orig, present)
    #[test]
    fn partial_result_and_state_rkyv_round_trip() {
        let make_account = |seed: u8| {
            let mut storage: EvmStorage = Default::default();
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

        let mut state: EvmState = Default::default();
        // Insert out of address order to verify the sort.
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
                gas: ResultGas::default().with_total_gas_spent(50_000),
                logs: vec![],
                output: Output::Call(Bytes::new()),
            },
            state,
        };

        let partial = PartialResultAndState::from(revm_ras);
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

        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&partial).unwrap();
        let recoded: PartialResultAndState =
            rkyv::from_bytes::<PartialResultAndState, rkyv::rancor::Error>(&bytes).unwrap();
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

        match (&partial.result, &recoded.result) {
            (ExecutionResult::Success { gas: a, .. }, ExecutionResult::Success { gas: b, .. }) => {
                assert_eq!(a.total_gas_spent(), b.total_gas_spent())
            }
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

    /// `PartialExecution::split` matrix: walks four shape configurations
    /// in one body and asserts each chunk's `expected_state` was threaded
    /// at the correct tx boundary.
    #[test]
    fn partial_execution_split_matrix() {
        // partials_per_block = 0 → empty.
        {
            let pe = make_partial_execution(1, &[10, 20, 30]);
            assert!(pe.split(0).is_empty());
        }

        // 4 txs, partials_per_block=2 → partial_size = ceil(4/2)=2, two chunks of 2.
        {
            let pe = make_partial_execution(1, &[10, 20, 30, 40]);
            let parts = pe.split(2);
            assert_eq!(parts.len(), 2);
            assert_eq!(parts[0].results.len(), 2);
            assert_eq!(parts[1].results.len(), 2);
        }

        // 5 txs, partials_per_block=2 → partial_size = ceil(5/2)=3, chunks(3) on
        // a 5-element vec yields lengths [3, 2].
        {
            let pe = make_partial_execution(1, &[1, 2, 3, 4, 5]);
            let parts = pe.split(2);
            assert_eq!(parts.len(), 2);
            assert_eq!(parts[0].results.len(), 3);
            assert_eq!(parts[1].results.len(), 2);
        }

        // 1 tx, partials_per_block=4 → ceil(1/4).max(1)=1, one chunk of 1.
        {
            let pe = make_partial_execution(1, &[42]);
            let parts = pe.split(4);
            assert_eq!(parts.len(), 1);
            assert_eq!(parts[0].results.len(), 1);
        }

        // 0 txs, partials_per_block=2 → empty.len().div_ceil(2).max(1)=1; chunks(1)
        // on an empty vec yields zero chunks. Confirms the "no work" branch.
        {
            let pe = make_partial_execution(1, &[]);
            let parts = pe.split(2);
            assert!(
                parts.is_empty(),
                "splitting an empty execution yields no chunks"
            );
        }

        // expected_state threading: each chunk's expected_state is the
        // running snapshot at its first tx. Mutate slot[0] in tx 0, then
        // check that chunk 1 (starting at tx 1) sees the post-mutation
        // snapshot.
        {
            let mut pe = make_partial_execution(1, &[10, 20]);
            // Override tx 0's result so it writes to EXPECTED_STORAGE_SLOTS[0].
            pe.results[0] = stub_result_with_l1_block_slot_change(
                10,
                EXPECTED_STORAGE_SLOTS[0],
                U256::ZERO,
                U256::from(0xCAFE),
            );
            let parts = pe.split(2);
            assert_eq!(parts.len(), 2);
            // Chunk 0 sees pre-tx-0 snapshot — slot[0] = 0.
            let ch0_slot0 = parts[0].expected_state[0]
                .account
                .storage
                .iter()
                .find(|s| s.slot == EXPECTED_STORAGE_SLOTS[0])
                .unwrap();
            assert_eq!(ch0_slot0.value, U256::ZERO);
            // Chunk 1 sees post-tx-0 snapshot — slot[0] = 0xCAFE.
            let ch1_slot0 = parts[1].expected_state[0]
                .account
                .storage
                .iter()
                .find(|s| s.slot == EXPECTED_STORAGE_SLOTS[0])
                .unwrap();
            assert_eq!(ch1_slot0.value, U256::from(0xCAFE));
        }
    }

    fn make_partial_execution(block_number: u64, gas_used_markers: &[u64]) -> PartialExecution {
        stub_chunk(block_number, gas_used_markers)
    }

    /// `PartialExecution::boot_info` produces the right shape: l1_head
    /// is the constant 0xFF mask, output roots = parent_hash, claimed
    /// block number = block_env.number - 1, chain_id passes through.
    #[test]
    fn partial_execution_boot_info_construction() {
        let mut pe = make_partial_execution(100, &[42]);
        pe.op_block_ctx.parent_hash = B256::repeat_byte(0xAA);
        let template = BootInfo {
            l1_head: B256::ZERO,
            agreed_l2_output_root: B256::ZERO,
            claimed_l2_output_root: B256::ZERO,
            claimed_l2_block_number: 0,
            chain_id: 999,
            rollup_config: Default::default(),
            l1_config: Default::default(),
        };
        let bi = pe.boot_info(&template);
        assert_eq!(bi.l1_head, L1_HEAD_TXN_ONLY_SENTINEL);
        assert_eq!(bi.agreed_l2_output_root, B256::repeat_byte(0xAA));
        assert_eq!(bi.claimed_l2_output_root, B256::repeat_byte(0xAA));
        assert_eq!(bi.claimed_l2_block_number, 99);
        assert_eq!(bi.chain_id, 999);

        // saturating_sub on block 0 → 0.
        let pe0 = make_partial_execution(0, &[42]);
        assert_eq!(pe0.boot_info(&template).claimed_l2_block_number, 0);
    }

    /// `PartialExecution::precondition_hash` is deterministic across calls
    /// and changes when any input field changes.
    #[test]
    fn partial_execution_precondition_hash_sensitivity() {
        let pe = make_partial_execution(1, &[10, 20]);
        assert_eq!(pe.precondition_hash(), pe.precondition_hash());

        let mut pe2 = pe.clone();
        pe2.tx_hashes[0] = B256::repeat_byte(0xFE);
        assert_ne!(pe.precondition_hash(), pe2.precondition_hash());

        let mut pe3 = pe.clone();
        pe3.block_env.number = U256::from(2);
        assert_ne!(pe.precondition_hash(), pe3.precondition_hash());

        let mut pe4 = pe.clone();
        pe4.expected_state[0].account.storage[0].value = U256::from(7);
        assert_ne!(pe.precondition_hash(), pe4.precondition_hash());
    }

    // ============================================================
    // witness.rs
    // ============================================================

    /// Witness rkyv round-trip preserves transactions verbatim, BlockEnv
    /// (PartialEq), OpBlockExecutionCtx field-wise, and the inserted cache
    /// entries (account + contract code).
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
            post_exec_mode: PostExecMode::Disabled,
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

        assert_eq!(witness.transactions, recoded.transactions);
        assert_eq!(witness.block_env, recoded.block_env);
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
    }

    /// `from_preflight` extracts envelopes whose hashes match
    /// `partial.tx_hashes` and bundles them into a witness. We use
    /// `gen_executions` from `executor::tests` (the same fixture
    /// witness.rs's tests use) and align tx_hashes to its envelopes.
    #[test]
    fn partial_execution_witness_from_preflight() {
        use crate::executor::tests::gen_executions;
        let executions = gen_executions(1);
        let execution = executions[0].as_ref();
        let envelopes = execution
            .attributes
            .transactions
            .as_deref()
            .expect("gen_executions populates transactions");
        let tx_hashes: Vec<B256> = envelopes
            .iter()
            .map(|e| B256::from_slice(SHA2::hash_bytes(e.as_ref()).as_bytes()))
            .collect();

        let mut pe = make_partial_execution(1, &vec![1u64; tx_hashes.len()]);
        pe.tx_hashes = tx_hashes;

        let witness = PartialExecutionWitness::from_preflight(pe.clone(), execution);
        assert_eq!(witness.transactions.len(), envelopes.len());
        for (txn, envelope) in witness.transactions.iter().zip(envelopes.iter()) {
            assert_eq!(txn, &envelope.to_vec());
        }
        // BlockEnv survives the bundle.
        assert_eq!(witness.block_env, pe.block_env);

        // Mismatched tx_hashes → zero transactions resolved.
        let mut pe_mismatch = pe.clone();
        for h in &mut pe_mismatch.tx_hashes {
            *h = B256::ZERO;
        }
        let mismatched = PartialExecutionWitness::from_preflight(pe_mismatch, execution);
        assert!(mismatched.transactions.is_empty());
    }

    /// `cache_results` matrix: walks every population branch:
    ///   - exists=true with code → account + contract populated
    ///   - exists=true without code → account only
    ///   - exists=false → CacheAccount::new_loaded_not_existing
    ///   - per-result LoadedAsNotExisting (account not yet in cache)
    ///   - per-result normal status (account not yet in cache, original_info seeded)
    ///   - per-result storage slot inserted
    #[test]
    fn cache_results_matrix() {
        // (1) exists=true with code.
        {
            let addr = address!("0xAA00000000000000000000000000000000000000");
            let code = Bytecode::new_raw(Bytes::from_static(&[0x60, 0x01]));
            let code_hash = code.hash_slow();
            let info = AccountInfo {
                nonce: 1,
                balance: U256::from(10),
                code_hash,
                code: Some(code.clone()),
                ..Default::default()
            };
            let expected = vec![ExpectedStateEntry {
                address: addr,
                account: ExpectedAccount {
                    exists: true,
                    info,
                    storage: vec![ExpectedStorageEntry {
                        slot: U256::from(1),
                        value: U256::from(7),
                    }],
                },
            }];
            let cache = cache_results(vec![], expected);
            assert!(cache.accounts.contains_key(&addr));
            assert!(cache.contracts.contains_key(&code_hash));
        }

        // (2) exists=true without code.
        {
            let addr = address!("0xBB00000000000000000000000000000000000000");
            let info = AccountInfo {
                nonce: 1,
                balance: U256::from(10),
                code_hash: B256::ZERO,
                code: None,
                ..Default::default()
            };
            let expected = vec![ExpectedStateEntry {
                address: addr,
                account: ExpectedAccount {
                    exists: true,
                    info,
                    storage: vec![],
                },
            }];
            let cache = cache_results(vec![], expected);
            assert!(cache.accounts.contains_key(&addr));
            assert!(cache.contracts.is_empty(), "no code → no contracts entry");
        }

        // (3) exists=false.
        {
            let addr = address!("0xCC00000000000000000000000000000000000000");
            let expected = vec![ExpectedStateEntry {
                address: addr,
                account: ExpectedAccount {
                    exists: false,
                    info: AccountInfo::default(),
                    storage: vec![],
                },
            }];
            let cache = cache_results(vec![], expected);
            // CacheAccount inserted, but as not_existing — no plain account.
            let entry = cache.accounts.get(&addr).expect("entry must exist");
            assert!(entry.account.is_none(), "exists=false → no plain account");
        }

        // (4) Per-result LoadedAsNotExisting + (5) per-result normal status +
        //     (6) per-result storage slot. All wired through one results vec
        //     so cache_results' result-loop branches are exercised.
        {
            let lana_addr = address!("0xDD00000000000000000000000000000000000000");
            let normal_addr = address!("0xEE00000000000000000000000000000000000000");
            let lana_account = PartialAccount {
                info: AccountInfo::default(),
                original_info: AccountInfo::default(),
                transaction_id: 0,
                storage: vec![],
                status: AccountStatus::LoadedAsNotExisting,
            };
            let normal_account = PartialAccount {
                info: AccountInfo {
                    nonce: 7,
                    ..Default::default()
                },
                original_info: AccountInfo {
                    nonce: 5,
                    ..Default::default()
                },
                transaction_id: 0,
                storage: vec![PartialStorageEntry {
                    slot: U256::from(1),
                    slot_value: EvmStorageSlot::new_changed(U256::from(11), U256::from(22), 0),
                }],
                status: AccountStatus::Touched,
            };
            let result = PartialResultAndState {
                result: ExecutionResult::Success {
                    reason: SuccessReason::Return,
                    gas: ResultGas::default().with_total_gas_spent(1),
                    logs: vec![],
                    output: Output::Call(Bytes::new()),
                },
                state: vec![
                    PartialStateEntry {
                        address: lana_addr,
                        account: lana_account,
                    },
                    PartialStateEntry {
                        address: normal_addr,
                        account: normal_account,
                    },
                ],
            };
            let cache = cache_results(vec![result], vec![]);
            // LANA address present as not_existing.
            let lana_entry = cache.accounts.get(&lana_addr).unwrap();
            assert!(lana_entry.account.is_none());
            // Normal address has plain account with seeded original_info.
            let normal_entry = cache.accounts.get(&normal_addr).unwrap();
            let plain = normal_entry
                .account
                .as_ref()
                .expect("plain account present");
            // original_info nonce was 5 (the `original_info` field of PartialAccount).
            assert_eq!(plain.info.nonce, 5);
            // Storage merged at original_value = 11.
            assert_eq!(
                plain.storage.get(&U256::from(1)).copied(),
                Some(U256::from(11))
            );
        }
    }

    /// Re-deriving `expected_state` against a `State` backed by the witness's
    /// cache yields the same hash as the host computed before encoding. This
    /// is the load-bearing property for `PartialExecution::precondition_hash`
    /// to round-trip through the witness/cache layer.
    #[test]
    fn witness_cache_round_trips_expected_state() {
        use crate::evm::expected::capture_required_expected_state;
        use crate::precondition::evm::hash_expected_state;
        use alloy_evm::revm::database::State;

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
}

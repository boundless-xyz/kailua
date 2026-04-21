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

//! Re-exports for host-side EVM capture.
//!
//! The cached-EVM wrapper types used for both chunk replay and per-transaction
//! `ResultAndState` capture live in `kailua-kona`. This module re-exports them
//! under the prover's namespace for callers that previously depended on
//! `kailua_prover::evm`.

pub use kailua_kona::evm::cached::{CachedEvm, CachedEvmFactory, TransactionResultCollector};

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_evm::op_revm::{OpHaltReason, OpSpecId, OpTransaction};
    use alloy_evm::revm::context::result::ResultAndState;
    use alloy_evm::revm::context::{BlockEnv, CfgEnv, TxEnv};
    use alloy_evm::revm::database::in_memory_db::InMemoryDB;
    use alloy_evm::revm::state::AccountInfo;
    use alloy_evm::{Evm, EvmEnv, EvmFactory};
    use alloy_primitives::{address, Address, Bytes, TxKind, U256};
    use std::sync::{Arc, Mutex};

    fn test_env() -> EvmEnv<OpSpecId> {
        let block_env = BlockEnv {
            number: U256::from(1u64),
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

    /// Construct a `CachedEvmFactory` in capture-only mode (empty chunk map +
    /// `Some(collector)`) so every `transact_raw` delegates to the inner `OpEvm`
    /// and the collector records the `ResultAndState`. Returns the factory and
    /// the shared collector Arc for later draining.
    fn capture_factory() -> (CachedEvmFactory, TransactionResultCollector) {
        let collector: TransactionResultCollector = Arc::new(Mutex::new(Vec::new()));
        let factory = CachedEvmFactory::new_with_traces(Vec::new(), Some(collector.clone()));
        (factory, collector)
    }

    #[test]
    fn transact_raw_captures_state_trace() {
        let (factory, _collector) = capture_factory();
        let mut db = InMemoryDB::default();

        let sender = address!("0x1000000000000000000000000000000000000000");
        let recipient = address!("0x2000000000000000000000000000000000000000");

        db.insert_account_info(
            sender,
            AccountInfo {
                balance: U256::from(1_000_000_000_000_000_000u128),
                nonce: 0,
                ..Default::default()
            },
        );

        let mut evm = factory.create_evm(db, test_env());
        let result = evm.transact_raw(make_transfer(sender, recipient, U256::from(1000), 0));
        assert!(result.is_ok());

        let block_traces = factory.take_all_block_traces();
        assert_eq!(block_traces.len(), 1, "should have one EVM slot");
        let traces = &block_traces[0];
        assert_eq!(traces.len(), 1, "should have exactly one trace entry");
        assert!(
            traces[0].state.contains_key(&sender),
            "trace should contain sender"
        );
        assert!(
            traces[0].state.contains_key(&recipient),
            "trace should contain recipient"
        );
    }

    #[test]
    fn system_call_does_not_append_trace() {
        let (factory, _collector) = capture_factory();
        let db = InMemoryDB::default();
        let mut evm = factory.create_evm(db, test_env());

        let caller = address!("0xdeaddeaddeaddeaddeaddeaddeaddeaddead0001");
        let contract = address!("0x000F3df6D732807Ef1319fB7B8bB8522d0Beac02");

        let _ = evm.transact_system_call(caller, contract, Bytes::new());

        let block_traces = factory.take_all_block_traces();
        // `create_evm` pushed a fresh slot, so block_traces has exactly one
        // inner Vec — which should be empty because system calls bypass the
        // per-tx capture path.
        assert_eq!(block_traces.len(), 1, "one EVM slot pushed by create_evm");
        assert_eq!(
            block_traces[0].len(),
            0,
            "system_call should not append to trace buffer"
        );
    }

    /// The factory's trace buffer is shared across clones via `Arc<Mutex<_>>` so that
    /// the kona executor (which clones the factory internally) accumulates into the
    /// same buffer the caller drains. This test exercises that sharing contract —
    /// confirming we inherit it correctly from `kailua-kona`.
    #[test]
    fn take_all_block_traces_drains_shared_buffer_across_clones() {
        let (factory, _collector) = capture_factory();
        let cloned = factory.clone();
        let mut db = InMemoryDB::default();

        let sender = address!("0x1000000000000000000000000000000000000000");
        let recipient = address!("0x2000000000000000000000000000000000000000");
        db.insert_account_info(
            sender,
            AccountInfo {
                balance: U256::from(1_000_000_000_000_000_000u128),
                nonce: 0,
                ..Default::default()
            },
        );

        let mut evm = cloned.create_evm(db, test_env());
        evm.transact_raw(make_transfer(sender, recipient, U256::from(1), 0))
            .unwrap();

        // Draining via the original factory observes the clone's traces because
        // the `block_traces` Arc is shared. One slot pushed by `create_evm`,
        // containing one entry from the one `transact_raw` call.
        let drained = factory.take_all_block_traces();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].len(), 1);
        assert!(cloned.take_all_block_traces().is_empty());

        // Silence unused warning for a halt reason import that's handy to have at
        // module scope for future tests.
        let _: Option<ResultAndState<OpHaltReason>> = None;
    }
}

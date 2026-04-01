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

//! Tracing EVM wrapper for capturing per-transaction state changes.
//!
//! [`TracingOpEvm`] wraps any `E: Evm` and captures `ResultAndState.state` after each
//! successful `transact_raw()` call into a shared trace buffer. System calls
//! (`transact_system_call`) are delegated transparently without appending to the trace buffer,
//! since they represent block-level prelude/epilogue work rather than ordered transaction-body
//! execution.
//!
//! [`TracingEvmFactory`] wraps `OpEvmFactory` and produces `TracingOpEvm<OpEvm<...>>` instances
//! that share a single trace buffer. Used on the host to capture per-transaction traces during
//! `build_block()` for chunk witness construction. Callers must drain the shared buffer via
//! [`TracingEvmFactory::take_traces`] at the per-block boundary so stale traces do not leak into
//! later witness construction.

use alloy_evm::op_revm::{OpContext, OpHaltReason, OpSpecId, OpTransaction, OpTransactionError};
use alloy_evm::precompiles::PrecompilesMap;
use alloy_evm::revm::context::BlockEnv;
use alloy_evm::revm::context::TxEnv;
use alloy_evm::revm::context_interface::result::{EVMError, ResultAndState};
use alloy_evm::revm::inspector::NoOpInspector;
use alloy_evm::revm::state::EvmState;
use alloy_evm::revm::Inspector;
use alloy_evm::{Database, Evm, EvmEnv, EvmFactory};
use alloy_op_evm::{OpEvm, OpEvmFactory};
use alloy_primitives::{Address, Bytes};
use std::mem;
use std::sync::{Arc, Mutex};

/// EVM wrapper that captures per-transaction `EvmState` traces on successful `transact_raw()`.
///
/// Follows the proven `CustomEvm` wrapper pattern: implements the `Evm` trait by delegating all
/// required methods to the inner EVM. No `Deref`/`DerefMut` needed — the block executor accesses
/// fields through `Evm` trait default methods (`db()`, `db_mut()`, etc.) which delegate to
/// `components()`/`components_mut()`.
pub struct TracingOpEvm<E: Evm> {
    inner: E,
    traces: Arc<Mutex<Vec<EvmState>>>,
}

impl<E: Evm> TracingOpEvm<E> {
    /// Creates a new tracing wrapper around the given EVM with a shared trace buffer.
    pub fn new(inner: E, traces: Arc<Mutex<Vec<EvmState>>>) -> Self {
        Self { inner, traces }
    }
}

impl<E: Evm> Evm for TracingOpEvm<E> {
    type DB = E::DB;
    type Tx = E::Tx;
    type Error = E::Error;
    type HaltReason = E::HaltReason;
    type Spec = E::Spec;
    type BlockEnv = E::BlockEnv;
    type Precompiles = E::Precompiles;
    type Inspector = E::Inspector;

    fn block(&self) -> &Self::BlockEnv {
        self.inner.block()
    }

    fn chain_id(&self) -> u64 {
        self.inner.chain_id()
    }

    /// Executes a transaction, captures the resulting `EvmState` on success, then returns
    /// the original result unmodified.
    fn transact_raw(
        &mut self,
        tx: Self::Tx,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        let result = self.inner.transact_raw(tx)?;
        self.traces.lock().unwrap().push(result.state.clone());
        Ok(result)
    }

    /// Delegates system calls to the inner EVM without appending to the tx-body trace buffer.
    /// System calls represent block-level prelude/epilogue work (beacon root, blockhashes,
    /// Canyon deployer), not ordered transaction-body execution.
    fn transact_system_call(
        &mut self,
        caller: Address,
        contract: Address,
        data: Bytes,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        self.inner.transact_system_call(caller, contract, data)
    }

    fn finish(self) -> (Self::DB, EvmEnv<Self::Spec, Self::BlockEnv>)
    where
        Self: Sized,
    {
        self.inner.finish()
    }

    fn set_inspector_enabled(&mut self, enabled: bool) {
        self.inner.set_inspector_enabled(enabled)
    }

    fn components(&self) -> (&Self::DB, &Self::Inspector, &Self::Precompiles) {
        self.inner.components()
    }

    fn components_mut(&mut self) -> (&mut Self::DB, &mut Self::Inspector, &mut Self::Precompiles) {
        self.inner.components_mut()
    }
}

/// Factory that wraps `OpEvmFactory` and produces `TracingOpEvm<OpEvm<...>>` instances
/// sharing a single trace buffer.
///
/// On the host, callers provide this factory to `KonaExecutor` to capture per-transaction
/// traces during `build_block()`. In the guest, callers use `OpEvmFactory` directly (zero
/// tracing overhead). The shared buffer must be drained with [`take_traces`](Self::take_traces)
/// after each block so traces from prior executions do not accumulate.
#[derive(Clone, Debug)]
pub struct TracingEvmFactory {
    inner: OpEvmFactory,
    traces: Arc<Mutex<Vec<EvmState>>>,
}

impl TracingEvmFactory {
    /// Creates a new tracing factory with a fresh shared trace buffer.
    pub fn new() -> Self {
        Self {
            inner: OpEvmFactory::default(),
            traces: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Atomically drains and returns the accumulated traces for the current block execution.
    ///
    /// This establishes the per-block trace boundary expected by chunk witness construction.
    /// Subsequent executions that reuse this factory start with an empty trace buffer.
    pub fn take_traces(&self) -> Vec<EvmState> {
        mem::take(&mut *self.traces.lock().unwrap())
    }
}

impl Default for TracingEvmFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl EvmFactory for TracingEvmFactory {
    type Evm<DB: Database, I: Inspector<OpContext<DB>>> =
        TracingOpEvm<OpEvm<DB, I, PrecompilesMap>>;
    type Context<DB: Database> = OpContext<DB>;
    type Tx = OpTransaction<TxEnv>;
    type Error<DBError: core::error::Error + Send + Sync + 'static> =
        EVMError<DBError, OpTransactionError>;
    type HaltReason = OpHaltReason;
    type Spec = OpSpecId;
    type BlockEnv = BlockEnv;
    type Precompiles = PrecompilesMap;

    fn create_evm<DB: Database>(
        &self,
        db: DB,
        input: EvmEnv<OpSpecId>,
    ) -> Self::Evm<DB, NoOpInspector> {
        TracingOpEvm::new(self.inner.create_evm(db, input), self.traces.clone())
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        input: EvmEnv<OpSpecId>,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        TracingOpEvm::new(
            self.inner.create_evm_with_inspector(db, input, inspector),
            self.traces.clone(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_evm::revm::context::CfgEnv;
    use alloy_evm::revm::database::in_memory_db::InMemoryDB;
    use alloy_evm::revm::state::AccountInfo;
    use alloy_primitives::{address, TxKind, U256};

    fn test_env() -> EvmEnv<OpSpecId> {
        let block_env = BlockEnv {
            number: U256::from(1),
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

    #[test]
    fn transact_raw_captures_state_trace() {
        let factory = TracingEvmFactory::new();
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

        let traces = factory.take_traces();
        assert_eq!(traces.len(), 1, "should have exactly one trace entry");
        assert!(
            traces[0].contains_key(&sender),
            "trace should contain sender"
        );
        assert!(
            traces[0].contains_key(&recipient),
            "trace should contain recipient"
        );
    }

    #[test]
    fn multiple_transactions_produce_ordered_traces() {
        let factory = TracingEvmFactory::new();
        let mut db = InMemoryDB::default();

        let sender = address!("0x1000000000000000000000000000000000000000");
        let recipient1 = address!("0x2000000000000000000000000000000000000000");
        let recipient2 = address!("0x3000000000000000000000000000000000000000");

        db.insert_account_info(
            sender,
            AccountInfo {
                balance: U256::from(1_000_000_000_000_000_000u128),
                nonce: 0,
                ..Default::default()
            },
        );

        let mut evm = factory.create_evm(db, test_env());
        evm.transact_commit(make_transfer(sender, recipient1, U256::from(1000), 0))
            .unwrap();
        evm.transact_commit(make_transfer(sender, recipient2, U256::from(2000), 1))
            .unwrap();

        let traces = factory.take_traces();
        assert_eq!(traces.len(), 2, "should have two trace entries");
        assert!(
            traces[0].contains_key(&recipient1),
            "first trace should contain recipient1"
        );
        assert!(
            traces[1].contains_key(&recipient2),
            "second trace should contain recipient2"
        );
    }

    #[test]
    fn system_call_does_not_append_trace() {
        let factory = TracingEvmFactory::new();
        let db = InMemoryDB::default();
        let mut evm = factory.create_evm(db, test_env());

        let caller = address!("0xdeaddeaddeaddeaddeaddeaddeaddeaddead0001");
        let contract = address!("0x000F3df6D732807Ef1319fB7B8bB8522d0Beac02");

        let _ = evm.transact_system_call(caller, contract, Bytes::new());

        let traces = factory.take_traces();
        assert_eq!(
            traces.len(),
            0,
            "system_call should not append to trace buffer"
        );
    }

    #[test]
    fn failed_transaction_does_not_append_trace() {
        let factory = TracingEvmFactory::new();
        let db = InMemoryDB::default();
        let mut evm = factory.create_evm(db, test_env());

        // Unfunded sender — should fail
        let tx = make_transfer(
            address!("0x0000000000000000000000000000000000000099"),
            address!("0x0000000000000000000000000000000000000001"),
            U256::from(1_000_000),
            0,
        );

        let result = evm.transact_raw(tx);
        assert!(
            result.is_err(),
            "transaction from unfunded account should fail"
        );

        let traces = factory.take_traces();
        assert_eq!(
            traces.len(),
            0,
            "failed transaction should not append to trace buffer"
        );
    }

    #[test]
    fn trait_method_field_access_works() {
        let factory = TracingEvmFactory::new();
        let db = InMemoryDB::default();
        let evm = factory.create_evm(db, test_env());

        assert_eq!(evm.block().number, U256::from(1));
        assert_eq!(evm.chain_id(), 1);
        let _ = evm.db();
    }

    #[test]
    fn take_traces_drains_shared_buffer() {
        let factory = TracingEvmFactory::new();
        let cloned_factory = factory.clone();
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

        let mut evm = cloned_factory.create_evm(db, test_env());
        evm.transact_raw(make_transfer(sender, recipient, U256::from(1000), 0))
            .unwrap();

        let traces = factory.take_traces();
        assert_eq!(
            traces.len(),
            1,
            "take_traces should return the accumulated block traces"
        );
        assert!(
            cloned_factory.take_traces().is_empty(),
            "take_traces should drain the shared buffer for all factory clones"
        );
    }
}

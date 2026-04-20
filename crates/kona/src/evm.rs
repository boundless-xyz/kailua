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

//! Chunking EVM wrapper for injecting pre-computed per-transaction results.
//!
//! [`ChunkingEvm`] wraps any `E: Evm` and intercepts `transact_raw()` to return pre-computed
//! [`ResultAndState`] entries (captured by `TracingEvmFactory` on the host) for transactions
//! covered by the supplied [`Chunk`] set, instead of re-executing them.
//!
//! [`ChunkingEvmFactory`] wraps [`OpEvmFactory`] and dispenses [`ChunkingEvm`] instances
//! keyed by the target block number (`input.block_env.number`). This lets the aggregation
//! guest run blocks through the standard `CachedExecutor` → `StatelessL2Builder::build_block`
//! → `OpBlockExecutor` pipeline, substituting pre-computed chunk results for the tx-body
//! transactions while prelude and epilogue continue to execute normally. The resulting
//! `BlockBuildingOutcome` (state root, receipts root, gas used) is byte-exact with
//! monolithic execution.

use crate::executor::Chunk;
use alloy_evm::op_revm::{OpContext, OpHaltReason, OpSpecId, OpTransaction};

use alloy_evm::precompiles::PrecompilesMap;
use alloy_evm::revm::context::{BlockEnv, TxEnv};
use alloy_evm::revm::context_interface::result::{EVMError, ResultAndState};
use alloy_evm::revm::inspector::NoOpInspector;
use alloy_evm::revm::Inspector;
use alloy_evm::{Database, Evm, EvmEnv, EvmFactory};
use alloy_op_evm::{OpEvm, OpEvmFactory, OpTxError};
use alloy_primitives::{Address, Bytes};
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::{Arc, Mutex};

/// Shared trace buffer keyed by L2 block number. Used by [`ChunkingEvmFactory`] and
/// [`run_core_client`](crate::client::core::run_core_client) to optionally capture
/// per-block `ResultAndState` traces during a monolithic pass so a subsequent run can
/// be replayed with `Chunk` entries built from the captured data. Alias exists to
/// keep the clippy `type_complexity` lint happy across call sites.
pub type ChunkTraceCollector = Arc<Mutex<HashMap<u64, Vec<ResultAndState<OpHaltReason>>>>>;

/// EVM wrapper that serves pre-computed `ResultAndState` entries from a set of
/// [`Chunk`]s covering this block's ordered transaction body.
pub struct ChunkingEvm<E: Evm> {
    /// Actual EVM implementation
    inner: E,
    /// Remaining chunks for this block in *reverse* execution order — `last()` is the
    /// currently-active chunk, `pop()` discards it once exhausted.
    chunks: Vec<Chunk>,
    /// Cursor into `chunks.last().results`. Reset to 0 whenever a chunk is exhausted
    /// and popped off.
    cursor: usize,
    /// Block number this EVM is executing. Set at construction time from
    /// `input.block_env.number`; used as the key when appending captured
    /// `ResultAndState` into `block_traces` below (test/host trace capture).
    block_number: u64,
    /// Optional per-block trace collector. When `Some`, every successful
    /// `transact_raw()` pushes its `ResultAndState` into
    /// `block_traces.lock()[&block_number]` (Vec created on demand). This is the
    /// same capture semantic as `TracingEvm`, but folded directly into
    /// `ChunkingEvm` so a single factory can be configured for capture during a
    /// monolithic run (empty chunks map → every call delegates to inner OpEvm and
    /// is captured as ground truth) and for replay during a second run (chunks
    /// populated → replayed `ResultAndState` entries served from `chunk.results`).
    block_traces: Option<ChunkTraceCollector>,
}

impl<E: Evm> ChunkingEvm<E> {
    /// Wraps `inner` and prepares the chunk cursor, optionally attaching a per-block
    /// trace collector. `chunks` must be in ascending execution order (matching
    /// `tx_start`); this constructor reverses the vec so `pop()` yields the next chunk.
    ///
    /// When `block_traces` is `Some`, each successful `transact_raw()` appends its
    /// `ResultAndState` into `block_traces[block_number]`. Used by the integration test
    /// harness to capture ground-truth traces from a monolithic `run_core_client` run
    /// (empty chunks → `ChunkingEvm` delegates to inner `OpEvm`, results are captured
    /// exactly as `TracingEvm` would).
    pub fn new_with_traces(
        inner: E,
        mut chunks: Vec<Chunk>,
        block_number: u64,
        block_traces: Option<ChunkTraceCollector>,
    ) -> Self {
        chunks.reverse();
        Self {
            inner,
            chunks,
            cursor: 0,
            block_number,
            block_traces,
        }
    }
}

impl<E: Evm<HaltReason = OpHaltReason>> Evm for ChunkingEvm<E>
where
    E::DB: alloy_evm::revm::Database,
{
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

    /// Returns the next pre-computed `ResultAndState` if an available chunk still has
    /// results to serve; otherwise delegates to the inner EVM.
    ///
    /// # Binding of replayed results to the chunk proof
    ///
    /// Each returned `ResultAndState` is taken verbatim from `chunk.results`, but the
    /// chunk is authenticated by [`crate::client::stitching::stitch_chunks`] through
    /// the six-input `chunk_trace`:
    ///
    /// ```text
    /// chunk_trace = SHA256(tx_hash || pre_db_hash || post_db_hash
    ///                      || pre_evm_hash || post_evm_hash || results_hash)
    /// ```
    ///
    /// where `results_hash` is the canonical SHA256 of the per-tx `ResultAndState`
    /// trace (see [`crate::precondition::chunking::hash_results`]). The chunk guest
    /// captures its own trace via `TracingOpEvmFactory` during execution and folds
    /// the hash into its journal. The aggregation side recomputes `hash_results`
    /// over `chunk.results` and uses the same formula to reconstruct the expected
    /// chunk journal — so any tampering with `chunk.results` (reorder,
    /// substitution, forged value, variant change) produces a mismatching journal
    /// and `env::verify()` rejects the chunk proof's assumption. The replayed
    /// `ResultAndState` entries are therefore cryptographically bound to the
    /// authenticated chunk proof; `ChunkingEvm` is safe to use them without
    /// per-call verification.
    fn transact_raw(
        &mut self,
        tx: Self::Tx,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        // Drop the active chunk (`chunks.last()`) once its results are exhausted,
        // resetting the cursor for whatever chunk comes next.
        if let Some(chunk) = self.chunks.last() {
            if self.cursor >= chunk.results.len() {
                self.chunks.pop();
                self.cursor = 0;
            }
        }

        // Serve from the active chunk when possible.
        let result = if let Some(chunk) = self.chunks.last() {
            let out = chunk.results[self.cursor].clone();
            self.cursor += 1;

            // Pre-load every account referenced by the replayed state diff into
            // the database's cache. `State<TrieDB>::commit` panics if the
            // executor later tries to mutate an account that was never loaded
            // via a `basic()` / `storage()` call (revm's `apply_account_state`
            // requires the address to be present). In the monolithic path
            // these reads happen naturally as the EVM executes; here we skip
            // execution and return pre-computed state, so we must ensure every
            // touched account — and every touched storage slot — is cache-
            // resident before returning. This restores the invariant that the
            // executor's commit step can rely on without behaving differently
            // between the monolithic and chunked paths.
            //
            // NOTE: `Database` here is `alloy_evm::revm::Database` (re-exported
            // from revm), which is the trait whose `basic`/`storage` methods
            // State<TrieDB> implements. The outer `Database` imported at the top
            // of this file is `alloy_evm::Database`, a thinner wrapper.
            use alloy_evm::revm::Database as RevmDatabase;
            let db = self.inner.db_mut();
            for (addr, account) in out.state.iter() {
                let _ = RevmDatabase::basic(db, *addr);
                for slot in account.storage.keys() {
                    let _ = RevmDatabase::storage(db, *addr, *slot);
                }
            }

            Ok(out)
        } else {
            self.inner.transact_raw(tx)
        };

        // Optional capture: when a per-block trace collector was attached via
        // `new_with_traces`, append the ResultAndState keyed by this block's
        // number. The chunk replay path (served from chunk.results) also
        // captures — benign since the captured trace equals the authenticated
        // `chunk.results` and callers choose whether to consume the buffer.
        if let (Ok(r), Some(traces)) = (&result, &self.block_traces) {
            traces
                .lock()
                .unwrap()
                .entry(self.block_number)
                .or_default()
                .push(r.clone());
        }
        result
    }

    /// Delegates system calls to the inner EVM. Block-level prelude and epilogue work
    /// (beacon root, blockhash ring, Canyon deployer, post-block balance increments)
    /// runs through the inner OpEvm unchanged and does not consume chunk results.
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

/// Factory that wraps `OpEvmFactory` and dispenses [`ChunkingEvm`] instances seeded with
/// per-block chunk data keyed by `input.block_env.number`.
///
/// The chunk map is consumed destructively per block: `create_evm` removes the entry for
/// the target block so repeated calls for the same block would receive an empty chunk
/// vec (and therefore fall through to the inner EVM). In practice each block creates a
/// fresh `ChunkingEvm` via one `create_evm` call from `StatelessL2Builder::build_block`.
///
/// Blocks without chunk data (no entry in the map) get a `ChunkingEvm` with an empty
/// chunk vec, which is semantically identical to the plain `OpEvm` path — every
/// `transact_raw()` delegates to the inner.
#[derive(Clone, Debug)]
pub struct ChunkingEvmFactory {
    inner: OpEvmFactory,
    /// Per-block chunk data keyed by block number. Arc<Mutex<...>> because `EvmFactory`
    /// requires `Clone + Send + Sync + 'static` and the factory is cloned by the
    /// executor pipeline.
    chunks: Arc<Mutex<HashMap<u64, Vec<Chunk>>>>,
    /// Optional per-block `ResultAndState` trace collector threaded into every
    /// `ChunkingEvm` this factory produces. When `Some`, each `ChunkingEvm`
    /// appends successful `transact_raw()` results into
    /// `block_traces[block_number]`. Used by the integration test harness to
    /// capture ground-truth traces during a monolithic run (empty chunks) and
    /// then rebuild `Chunk` entries for a subsequent replay run.
    block_traces: Option<ChunkTraceCollector>,
}

impl ChunkingEvmFactory {
    /// Constructs a factory with the given block-keyed chunk data. The map is consumed
    /// destructively on each `create_evm` call.
    pub fn new(chunks: HashMap<u64, Vec<Chunk>>) -> Self {
        Self {
            inner: OpEvmFactory::default(),
            chunks: Arc::new(Mutex::new(chunks)),
            block_traces: None,
        }
    }

    /// Variant of [`new`](Self::new) that also attaches a per-block trace collector.
    /// Every `ChunkingEvm` produced by this factory will append successful
    /// `transact_raw()` results into `block_traces[block_number]`. Drain via
    /// `block_traces.lock()` at the per-block boundary the caller chooses (see
    /// the integration test in `crates/kona/src/client/core.rs`).
    pub fn new_with_traces(
        chunks: HashMap<u64, Vec<Chunk>>,
        block_traces: Option<ChunkTraceCollector>,
    ) -> Self {
        Self {
            inner: OpEvmFactory::default(),
            chunks: Arc::new(Mutex::new(chunks)),
            block_traces,
        }
    }

    /// Removes and returns the chunks for the given block number, or an empty vec.
    fn take_chunks(&self, block_number: u64) -> Vec<Chunk> {
        self.chunks
            .lock()
            .unwrap()
            .remove(&block_number)
            .unwrap_or_default()
    }
}

impl EvmFactory for ChunkingEvmFactory {
    type Evm<DB: Database, I: Inspector<OpContext<DB>>> = ChunkingEvm<OpEvm<DB, I, PrecompilesMap>>;
    type Context<DB: Database> = OpContext<DB>;
    type Tx = OpTransaction<TxEnv>;
    type Error<DBError: core::error::Error + Send + Sync + 'static> = EVMError<DBError, OpTxError>;
    type HaltReason = OpHaltReason;
    type Spec = OpSpecId;
    type BlockEnv = BlockEnv;
    type Precompiles = PrecompilesMap;

    fn create_evm<DB: Database>(
        &self,
        db: DB,
        input: EvmEnv<OpSpecId>,
    ) -> Self::Evm<DB, NoOpInspector> {
        let block_number = input.block_env.number.to::<u64>();
        let chunks = self.take_chunks(block_number);
        ChunkingEvm::new_with_traces(
            self.inner.create_evm(db, input),
            chunks,
            block_number,
            self.block_traces.clone(),
        )
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        input: EvmEnv<OpSpecId>,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        let block_number = input.block_env.number.to::<u64>();
        let chunks = self.take_chunks(block_number);
        ChunkingEvm::new_with_traces(
            self.inner.create_evm_with_inspector(db, input, inspector),
            chunks,
            block_number,
            self.block_traces.clone(),
        )
    }
}

// ============================================================================
//  TracingEvm + TracingOpEvmFactory — capture per-tx ResultAndState
// ============================================================================
//
// Used by two independent paths:
//   1. The host (`kailua-prover`) to capture per-tx traces during monolithic
//      block pre-execution, which feeds chunk witness construction and the
//      `Chunk.results` field for aggregation.
//   2. The chunk guest (`run_core_client` chunk branch) to capture per-tx
//      traces during its own execution so `results_hash` can be folded into
//      `chunk_trace` — this is what binds the chunk proof to the exact
//      per-transaction execution trace rather than just the pre/post state
//      endpoints.

/// EVM wrapper that captures full per-transaction `ResultAndState` on successful
/// `transact_raw()`. System calls pass through transparently and are *not* appended to
/// the trace buffer (they represent block-level prelude/epilogue work, not ordered
/// transaction-body execution).
pub struct TracingEvm<E: Evm> {
    inner: E,
    traces: Arc<Mutex<Vec<ResultAndState<E::HaltReason>>>>,
}

impl<E: Evm> TracingEvm<E> {
    /// Wrap the given EVM with a shared trace buffer. Callers drain the buffer via
    /// [`TracingOpEvmFactory::take_traces`] at each per-block boundary.
    pub fn new(inner: E, traces: Arc<Mutex<Vec<ResultAndState<E::HaltReason>>>>) -> Self {
        Self { inner, traces }
    }
}

impl<E: Evm> Evm for TracingEvm<E> {
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

    /// Executes a transaction, captures the full `ResultAndState` on success, then
    /// returns the original result unmodified.
    fn transact_raw(
        &mut self,
        tx: Self::Tx,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        let result = self.inner.transact_raw(tx)?;
        self.traces.lock().unwrap().push(result.clone());
        Ok(result)
    }

    /// System calls flow through without being appended to the tx-body trace buffer.
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

/// Factory that wraps `OpEvmFactory` and produces [`TracingEvm`]-wrapped `OpEvm`
/// instances sharing a single trace buffer. Drain with [`take_traces`](Self::take_traces)
/// at each per-block boundary; the shared buffer is an `Arc<Mutex<_>>` so clones of the
/// factory (e.g., via `KonaExecutor`) accumulate into the same trace vec.
///
/// # Per-block boundary is a caller convention (review finding L-1)
///
/// The shared buffer has no intrinsic notion of "block boundary" — callers MUST
/// invoke [`take_traces`](Self::take_traces) between blocks to prevent interleaving.
/// In the zkVM (single-threaded) this is guaranteed by construction. On the host, if
/// a caller ever runs two blocks concurrently against clones of the same factory
/// (`Arc<Mutex>` shares the buffer), traces will interleave silently — the
/// `traces.len() == block_txs.len()` sanity check in the chunk guest catches a length
/// mismatch but not correct-length interleaving. The chunk guest always creates a
/// fresh factory per chunk (`TracingOpEvmFactory::new()` in `run_core_client`), so
/// the host-concurrency concern is confined to host witness construction. Document
/// and enforce convention there.
#[derive(Clone, Debug)]
pub struct TracingOpEvmFactory {
    inner: OpEvmFactory,
    traces: Arc<Mutex<Vec<ResultAndState<OpHaltReason>>>>,
}

impl TracingOpEvmFactory {
    /// Create a factory with a fresh shared trace buffer.
    pub fn new() -> Self {
        Self {
            inner: OpEvmFactory::default(),
            traces: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Atomically drain and return the accumulated traces. Subsequent executions
    /// start with an empty trace buffer.
    pub fn take_traces(&self) -> Vec<ResultAndState<OpHaltReason>> {
        std::mem::take(&mut *self.traces.lock().unwrap())
    }
}

impl Default for TracingOpEvmFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl EvmFactory for TracingOpEvmFactory {
    type Evm<DB: Database, I: Inspector<OpContext<DB>>> = TracingEvm<OpEvm<DB, I, PrecompilesMap>>;
    type Context<DB: Database> = OpContext<DB>;
    type Tx = OpTransaction<TxEnv>;
    type Error<DBError: core::error::Error + Send + Sync + 'static> = EVMError<DBError, OpTxError>;
    type HaltReason = OpHaltReason;
    type Spec = OpSpecId;
    type BlockEnv = BlockEnv;
    type Precompiles = PrecompilesMap;

    fn create_evm<DB: Database>(
        &self,
        db: DB,
        input: EvmEnv<OpSpecId>,
    ) -> Self::Evm<DB, NoOpInspector> {
        TracingEvm::new(self.inner.create_evm(db, input), self.traces.clone())
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        input: EvmEnv<OpSpecId>,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        TracingEvm::new(
            self.inner.create_evm_with_inspector(db, input, inspector),
            self.traces.clone(),
        )
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::precondition::chunking::EvmAccumulatorState;
    use alloy_evm::revm::context::CfgEnv;
    use alloy_evm::revm::context_interface::result::{ExecutionResult, Output, SuccessReason};
    use alloy_evm::revm::database::in_memory_db::InMemoryDB;
    use alloy_evm::revm::state::AccountInfo;
    use alloy_primitives::{address, keccak256, TxKind, B256, U256};

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

    fn stub_chunk(tag: u16, gas_used_markers: &[u64]) -> Chunk {
        Chunk {
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
            agreed_l2_output_root: keccak256(format!("agreed_out_{tag}")),
            parent_block_number: tag as u64,
            block_env: alloy_evm::revm::context::BlockEnv::default(),
            op_block_ctx: alloy_op_evm::block::OpBlockExecutionCtx::default(),
        }
    }

    /// A pre-computed chunk entry must be returned verbatim by `transact_raw()`,
    /// bypassing inner-EVM execution. The marker gas_used proves it came from the chunk.
    #[test]
    fn precomputed_results_returned_in_order() {
        let chunks = HashMap::from([(1u64, vec![stub_chunk(0, &[100_001, 100_002, 100_003])])]);
        let factory = ChunkingEvmFactory::new(chunks);

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
        let factory = ChunkingEvmFactory::new(HashMap::new());

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
        let factory = ChunkingEvmFactory::new(chunks);

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
        let factory = ChunkingEvmFactory::new(chunks);

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
        let factory = ChunkingEvmFactory::new(chunks);

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
        let factory = ChunkingEvmFactory::new(HashMap::new());
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
        let factory = ChunkingEvmFactory::new(chunks);

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
        let factory = ChunkingEvmFactory::new(chunks);

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

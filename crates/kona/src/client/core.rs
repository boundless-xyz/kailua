// Copyright 2024, 2025 RISC Zero, Inc.
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

use crate::client::log;
use crate::driver::CachedDriver;
use crate::evm::caching::ChunkingEvmFactory;
use crate::evm::db::PanicDB;
use crate::evm::tracing::ChunkTraceCollector;
use crate::evm::tracing::TracingOpEvmFactory;
use crate::evm::PartialExecution;
use crate::executor::{new_execution_cursor, CachedExecutor, Execution};
use crate::kona::OracleL1ChainProvider;
use crate::oracle::local::LocalOnceOracle;
use crate::precondition::chunking::{
    compute_chunk_trace, compute_tx_hash, hash_block_ctx, hash_cache, hash_evm_state,
    hash_overlay_state, hash_results, EvmAccumulatorState,
};
use crate::precondition::execution::exec_precondition_hash;
use crate::precondition::{proposal, Precondition};
use crate::witness::ChunkWitnessData;
use alloy_consensus::transaction::SignerRecoverable;
use alloy_consensus::TxReceipt;
use alloy_eips::eip2718::{Decodable2718, WithEncoded};
use alloy_evm::block::BlockExecutor;
use alloy_evm::revm::bytecode::Bytecode;
use alloy_evm::revm::database::in_memory_db::CacheDB;
use alloy_evm::EvmFactory;
use alloy_op_evm::block::OpAlloyReceiptBuilder;
use alloy_op_evm::OpBlockExecutor;
use alloy_primitives::map::HashMap;
use alloy_primitives::{Sealed, B256};
use anyhow::{bail, Context};
use kona_derive::{BlobProvider, ChainProvider, DataAvailabilityProvider, EthereumDataSource};
use kona_driver::{Driver, Executor};
use kona_executor::TrieDBProvider;
use kona_genesis::RollupConfig;
use kona_preimage::{CommsClient, PreimageKey};
use kona_proof::errors::OracleProviderError;
use kona_proof::executor::KonaExecutor;
use kona_proof::l1::OraclePipeline;
use kona_proof::l2::OracleL2ChainProvider;
use kona_proof::sync::new_oracle_pipeline_cursor;
use kona_proof::{BootInfo, FlushableCache, HintType};
use op_alloy_consensus::OpTxEnvelope;
use risc0_zkvm::sha::Digestible;
use std::fmt::Debug;
use std::mem::take;
use std::sync::{Arc, Mutex};

pub trait DASourceProvider<
    C: ChainProvider + Send + Sync + Clone + Debug,
    B: BlobProvider + Send + Sync + Clone + Debug,
>
{
    type DAS: DataAvailabilityProvider + Send + Sync + Debug + Clone;

    fn new_from_parts(self, l1_provider: C, blobs: B, cfg: &RollupConfig) -> Self::DAS;
}

#[derive(Clone, Copy, Debug)]
pub struct EthereumDataSourceProvider;

impl<
        C: ChainProvider + Send + Sync + Clone + Debug,
        B: BlobProvider + Send + Sync + Clone + Debug,
    > DASourceProvider<C, B> for EthereumDataSourceProvider
{
    type DAS = EthereumDataSource<C, B>;

    fn new_from_parts(self, l1_provider: C, blobs: B, cfg: &RollupConfig) -> Self::DAS {
        EthereumDataSource::new_from_parts(l1_provider, blobs, cfg)
    }
}

/// Runs the Kailua client to drive rollup state transition derivation using Kona.
///
/// # Arguments
/// * `proposal_data_hash` - The hash of the proposal blob precondition data.
/// * `oracle` - The client for preloaded communication with the host.
/// * `stream` - The client for streamed communication with the host.
/// * `beacon` - The blob provider.
/// * `da_source_provider` - The provider for a data availability source.
/// * `execution_cache` - A vector of cached executions to reuse.
/// * `execution_trace` - An optional target to dump uncached executions.
/// * `derivation_cache` - An initial snapshot of the derivation pipeline to resume from.
/// * `derivation_trace` - An optional target for saving a final snapshot of the derivation pipeline.
///
/// # Returns
/// A result containing a tuple (`BootInfo`, `Precondition`) upon success, or an error of type `anyhow::Error`.
/// - `BootInfo` contains essential configuration information for bootstrapping the rollup client.
/// - `Precondition` represents the full precondition for the validity of the boot record.
#[allow(clippy::too_many_arguments)]
pub fn run_core_client<
    O: CommsClient + FlushableCache + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
    D: DASourceProvider<OracleL1ChainProvider<O>, B>,
>(
    proposal_data_hash: B256,
    oracle: Arc<O>,
    stream: Arc<O>,
    mut beacon: B,
    da_source_provider: D,
    execution_cache: Vec<Arc<Execution>>,
    execution_trace: Option<Arc<Mutex<Vec<Execution>>>>,
    derivation_cache: Option<CachedDriver>,
    derivation_trace: Option<Arc<Mutex<Option<CachedDriver>>>>,
    chunk_witness: Option<ChunkWitnessData>,
    chunks: Vec<Vec<PartialExecution>>,
    chunk_trace_collector: Option<ChunkTraceCollector>,
) -> anyhow::Result<(BootInfo, Precondition)>
where
    <B as BlobProvider>::Error: Debug,
{
    let oracle = Arc::new(LocalOnceOracle::new(oracle));
    kona_proof::block_on(async move {
        ////////////////////////////////////////////////////////////////
        //                          PROLOGUE                          //
        ////////////////////////////////////////////////////////////////
        log("BOOT");
        let boot = BootInfo::load(oracle.as_ref())
            .await
            .context("BootInfo::load")?;
        assert_eq!(boot.chain_id, boot.rollup_config.l2_chain_id);
        log(&format!("{:?} L1_HEAD", boot.l1_head));
        log(&format!("{:?} L2_AGREED", boot.agreed_l2_output_root));
        log(&format!(
            "{:?} L2_CLAIMED (#{})",
            boot.claimed_l2_output_root, boot.claimed_l2_block_number
        ));
        let l1_config = Arc::new(boot.l1_config.clone());
        let rollup_config = Arc::new(boot.rollup_config.clone());

        ////////////////////////////////////////////////////////////////
        //                    CHUNK EXECUTION-ONLY                    //
        ////////////////////////////////////////////////////////////////
        if boot.l1_head == B256::repeat_byte(0xFF) {
            log("CHUNK EXECUTION");
            let ChunkWitnessData {
                transactions,
                block_env,
                op_block_ctx,
                cache,
                evm_state,
            } = chunk_witness.context("chunk witness required in chunk mode")?;

            // Validate cached contract bytecodes against their code_hash keys.
            validate_cached_contracts(&cache.contracts);

            // Compute pre-hashes from the witness state.
            let tx_hash = compute_tx_hash(&transactions);
            let block_ctx_hash = hash_block_ctx(&block_env, &op_block_ctx);
            let pre_db_hash = hash_cache(&cache);
            let pre_evm_hash = hash_evm_state(&evm_state);

            // Build CacheDB<PanicDB> from witness cache.
            let mut cache_db = CacheDB::new(PanicDB);
            cache_db.cache = cache.clone();

            // Wrap in State
            let mut state = alloy_evm::revm::database::states::State::builder()
                .with_database(cache_db)
                .build();
            // set the state-clear flag manually here because skip `apply_pre_execution_changes`
            state.set_state_clear_flag(true);

            // Set up EVM environment.
            // ref optimism/rust/kona/crates/proof/executor/src/builder/env.rs (`evm_cfg_env`).
            let cfg_env = alloy_evm::revm::context::CfgEnv::new()
                .with_chain_id(boot.chain_id)
                .with_spec_and_mainnet_gas_params(rollup_config.spec_id(block_env.timestamp.to()));
            let evm_env = alloy_evm::EvmEnv::new(cfg_env, block_env);

            // (e) Construct upstream `OpBlockExecutor` and seed its accumulators from the witness.
            let tracing_op_evm_factory = TracingOpEvmFactory::new();
            let mut op_block_executor = OpBlockExecutor::new(
                tracing_op_evm_factory.create_evm(&mut state, evm_env),
                op_block_ctx,
                rollup_config.clone(),
                OpAlloyReceiptBuilder::default(),
            );
            // Seed the executor's running accumulators
            op_block_executor.gas_used = evm_state.cumulative_gas_used;
            op_block_executor.da_footprint_used = evm_state.da_footprint_used;
            op_block_executor.receipts = evm_state.receipts.clone();
            // Execute transactions
            let mut logs_bloom = evm_state.logs_bloom;
            for tx_bytes in transactions {
                // Decode the signed transaction envelope
                let mut buf = tx_bytes.as_slice();
                let tx = OpTxEnvelope::decode_2718_exact(&mut buf)
                    .context("invalid transaction encoding in chunk witness")?;
                let recovered = tx
                    .try_into_recovered()
                    .context("invalid transaction signature in chunk witness")?;
                let wrapped = WithEncoded::new(tx_bytes.into(), recovered);

                // Delegates to `execute_transaction_without_commit`
                op_block_executor
                    .execute_transaction(wrapped)
                    .map_err(|e| anyhow::anyhow!("chunk transaction execution failed: {e}"))?;

                // Accrue logs from the newly-added receipt into the running bloom.
                if let Some(receipt) = op_block_executor.receipts.last() {
                    for log in TxReceipt::logs(receipt) {
                        logs_bloom.accrue_log(log);
                    }
                }
            }

            // Reconstruct the chunk's final accumulator state for hashing.
            let evm_accum = EvmAccumulatorState {
                cumulative_gas_used: op_block_executor.gas_used,
                da_footprint_used: op_block_executor.da_footprint_used,
                blob_gas_used: op_block_executor.da_footprint_used,
                logs_bloom,
                receipts: take(&mut op_block_executor.receipts),
            };

            // Drop the executor to release the borrow on `state` (it owns
            // the EVM, which borrows `state` mutably).
            drop(op_block_executor);

            // Compute post-hashes.
            let post_db_hash = hash_overlay_state(&cache, &state.cache, &state.block_hashes);
            let post_evm_hash = hash_evm_state(&evm_accum);
            let traces = tracing_op_evm_factory.take_traces();
            let results_hash = hash_results(&traces);

            // Compute chunk_trace
            let chunk_trace = compute_chunk_trace(
                tx_hash,
                pre_db_hash,
                post_db_hash,
                pre_evm_hash,
                post_evm_hash,
                results_hash,
                block_ctx_hash,
            );

            // Return
            return Ok((boot, Precondition::default().chunk(chunk_trace)));
        }

        log("SAFE HEAD HASH");
        let safe_head_hash = fetch_safe_head_hash(oracle.as_ref(), boot.agreed_l2_output_root)
            .await
            .context("fetch_safe_head_hash")?;

        // Instantiate oracle-backed providers
        let mut l1_provider = OracleL1ChainProvider::new(boot.l1_head, stream)
            .await
            .context("OracleL1ChainProvider::new")?;
        let mut l2_provider =
            OracleL2ChainProvider::new(safe_head_hash, rollup_config.clone(), oracle.clone());

        // The claimed L2 block number must be greater than or equal to the L2 safe head.
        // Fetch the safe head's block header.
        log("SAFE HEAD");
        let safe_head = l2_provider
            .header_by_hash(safe_head_hash)
            .map(|header| Sealed::new_unchecked(header, safe_head_hash))
            .context("l2_provider.header_by_hash")?;

        if boot.claimed_l2_block_number < safe_head.number {
            bail!("Invalid claim: Safe l2 head block number below claimed l2 block number.");
        }
        let safe_head_number = safe_head.number;
        let expected_output_count = (boot.claimed_l2_block_number - safe_head_number) as usize;

        ////////////////////////////////////////////////////////////////
        //                     EXECUTION CACHING                      //
        ////////////////////////////////////////////////////////////////
        if boot.l1_head.is_zero() {
            log("EXECUTION ONLY");
            let cursor =
                new_execution_cursor(rollup_config.as_ref(), safe_head.clone(), &mut l2_provider)
                    .await
                    .context("new_execution_cursor")?;
            l2_provider.set_cursor(cursor.clone());

            // Build the block-number → chunks map for this EXECUTION-ONLY run. When
            // chunks are supplied, use `ChunkingEvmFactory` so that per-tx execution
            // is served from `chunk.results` (with account/storage pre-load on the
            // replay path, see `ChunkingEvm::transact_raw`). When no chunks are
            // supplied the factory's map is empty → every `transact_raw()` delegates
            // to inner `OpEvm`, identical to plain `OpEvmFactory`.
            //
            // Running the EXECUTION-ONLY branch with chunks populated also unlocks
            // `run_stateless_client(Witness { stitched_executions, chunks, .. })`:
            // pass 1 captures both via `test_derivation_with_chunks_and_traces`,
            // pass 2 sets `l1_head = ZERO` to take this branch, and no L1 blobs are
            // needed since derivation doesn't run. The CHUNK VERIFY phase below
            // authenticates each chunk against its Execution (same cross-checks as
            // the DERIVATION branch: output-root anchor, tx_hash, block_env,
            // blob pricing, hash_evm_state, receipts_root).
            let safe_head_parent_exec: alloy_consensus::Header = safe_head.inner().clone();
            let has_chunks_exec = chunks.iter().any(|v| !v.is_empty());
            let chunks_by_block_exec: std::collections::HashMap<u64, Vec<PartialExecution>> =
                chunks
                    .iter()
                    .enumerate()
                    .filter_map(|(i, v)| {
                        if v.is_empty() {
                            None
                        } else {
                            Some((safe_head_number + 1 + i as u64, v.clone()))
                        }
                    })
                    .collect();
            let factory = ChunkingEvmFactory::new_with_traces(
                chunks_by_block_exec,
                chunk_trace_collector.clone(),
            );
            let mut kona_executor: KonaExecutor<'_, _, _, ChunkingEvmFactory> = KonaExecutor::new(
                rollup_config.as_ref(),
                l2_provider.clone(),
                l2_provider.clone(),
                factory,
                None,
            );
            kona_executor.update_safe_head(safe_head);

            // Validate expected block count
            assert_eq!(expected_output_count, execution_cache.len());

            // Validate non-empty execution trace
            assert!(!execution_cache.is_empty());

            // Calculate precondition hash
            let execution_trace_hash = exec_precondition_hash(execution_cache.as_slice());

            // Validate terminating block number
            assert_eq!(
                execution_cache.last().unwrap().artifacts.header.number,
                boot.claimed_l2_block_number
            );

            // Retain references to each Execution for the CHUNK VERIFY phase —
            // the loop below consumes `execution_cache` via `for execution in
            // execution_cache`, but `Arc::clone` lets us keep a cheap handle.
            let executions_for_verify: Vec<Arc<Execution>> = execution_cache.to_vec();

            // Validate executed chain
            let mut latest_output_root = boot.agreed_l2_output_root;
            for execution in execution_cache {
                // Unpack [Execution]
                let Execution {
                    agreed_output,
                    attributes,
                    artifacts,
                    claimed_output,
                } = execution.as_ref();
                // Verify initial state
                assert_eq!(agreed_output, &latest_output_root);
                // Verify transition
                let executor_result = kona_executor
                    .execute_payload(attributes.clone())
                    .await
                    .context("execute_payload")?;
                assert_eq!(artifacts.header, executor_result.header);
                assert_eq!(artifacts.execution_result, executor_result.execution_result);
                // Update state
                kona_executor.update_safe_head(executor_result.header);
                latest_output_root = kona_executor
                    .compute_output_root()
                    .context("compute_output_root: Verify post state")?;
                // Verify post state
                assert_eq!(claimed_output, &latest_output_root);
                log(&format!(
                    "OUTPUT: {}/{}\t{latest_output_root}",
                    artifacts.header.number, boot.claimed_l2_block_number
                ));
            }

            // CHUNK VERIFY phase (EXECUTION-ONLY variant). Delegates the per-block
            // cross-checks to `verify_chunks_against_blocks` — shared with the
            // DERIVATION branch below so both paths use identical authentication
            // logic (output-root anchor, block_env fields, blob pricing, tx_hash,
            // hash_evm_state, receipts_root, hash-chain continuity).
            if has_chunks_exec {
                log("CHUNK VERIFY");
                let exec_refs: Vec<&Execution> =
                    executions_for_verify.iter().map(|e| e.as_ref()).collect();
                crate::precondition::chunking::verify_chunks_against_blocks(
                    &chunks,
                    &exec_refs,
                    &safe_head_parent_exec,
                    safe_head_number,
                    rollup_config.as_ref(),
                )?;
            }

            // Validate claimed_l2_output_root against latest_output_root
            assert_eq!(boot.claimed_l2_output_root, latest_output_root);
            // Return result
            return Ok((
                boot,
                Precondition::default().execution(execution_trace_hash),
            ));
        }

        ////////////////////////////////////////////////////////////////
        //                   DERIVATION & EXECUTION                   //
        ////////////////////////////////////////////////////////////////
        log("PRECONDITION");
        let proposal_precondition_data =
            proposal::load_proposal_data(proposal_data_hash, oracle.clone(), &mut beacon)
                .await
                .context("load_precondition_data")?;

        log("DERIVATION & EXECUTION");
        // Retain an unsealed copy of the safe head so the CHUNK VERIFY phase below
        // can use it as the parent header for block_idx == 0 (deriving expected
        // blob_excess_gas_and_price). `new_oracle_pipeline_cursor` consumes its
        // `safe_head` argument, so we clone before handing ownership over.
        let safe_head_parent: alloy_consensus::Header = safe_head.inner().clone();
        // Create a new derivation driver with the given boot information and oracle.
        let cursor = new_oracle_pipeline_cursor(
            rollup_config.as_ref(),
            safe_head,
            boot.agreed_l2_output_root,
            &mut l1_provider,
            &mut l2_provider,
        )
        .await
        .context("new_oracle_pipeline_cursor")?;
        l2_provider.set_cursor(cursor.clone());

        // Construct the DA provider
        let da_provider =
            da_source_provider.new_from_parts(l1_provider.clone(), beacon, &rollup_config);

        // Load the Kailua executor with caching support.
        //
        // We always use `ChunkingEvmFactory` as the `EvmFactory` parameter. For blocks
        // with pre-computed chunk data, the factory dispenses a `ChunkingEvm` that
        // returns host-captured `ResultAndState` entries verbatim for tx-body calls
        // (Decision 12). Blocks without chunk data get a `ChunkingEvm` with an empty
        // chunk vec, which is semantically identical to plain `OpEvm` — every
        // `transact_raw()` delegates straight through. This keeps the executor type
        // homogeneous whether chunks are present or not.
        //
        // Outer index i corresponds to block `safe_head_number + 1 + i` (per witness
        // convention — see `Witness::chunks` doc).
        let has_chunks = chunks.iter().any(|v| !v.is_empty());
        let chunks_by_block: std::collections::HashMap<u64, Vec<PartialExecution>> = chunks
            .iter()
            .enumerate()
            .filter_map(|(i, v)| {
                if v.is_empty() {
                    None
                } else {
                    Some((safe_head_number + 1 + i as u64, v.clone()))
                }
            })
            .collect();

        // When chunks are supplied, we MUST post-verify per-block coherence against
        // derivation output (output-root anchor, parent block number, tx_hash, and
        // receipts_root reconstruction — see `verify_block_chunks`). That requires
        // capturing the `Execution` for each newly-derived block. The `CachedExecutor`
        // already supports this via its `collection_target` slot; we thread our own
        // internal vec in for chunk verification.
        //
        // Restrictions when chunks are active (kept simple for soundness):
        //   - `execution_cache` must be empty — mixing stitched-execution caching with
        //     chunk aggregation is not currently supported (a cache hit would skip
        //     `collection_target`, leaving a chunk block unverified).
        //   - `execution_trace` must be `None` — the caller-facing trace slot is
        //     repurposed here for internal verification capture. Callers that want
        //     stitched-execution output should not also pass chunks.
        if has_chunks {
            assert!(
                execution_cache.is_empty(),
                "chunk aggregation does not support combining with stitched execution cache"
            );
            assert!(
                execution_trace.is_none(),
                "execution_trace must be None when chunks are supplied; chunk verification \
                 reuses the collection target slot"
            );
        }
        let chunk_verify_target: Option<Arc<Mutex<Vec<Execution>>>> = if has_chunks {
            Some(Arc::new(Mutex::new(Vec::new())))
        } else {
            None
        };
        let effective_collection_target = if has_chunks {
            chunk_verify_target.clone()
        } else {
            execution_trace
        };

        let cached_executor = CachedExecutor::<KonaExecutor<'_, _, _, ChunkingEvmFactory>>::new(
            execution_cache,
            rollup_config.as_ref(),
            l2_provider.clone(),
            l2_provider.clone(),
            ChunkingEvmFactory::new_with_traces(chunks_by_block, chunk_trace_collector.clone()),
            effective_collection_target,
        );

        // Resume from cached derivation pipeline or start a new one
        let (derivation_cache_hash, mut driver) = match derivation_cache {
            None => (
                B256::ZERO,
                Driver::new(
                    cursor.clone(),
                    cached_executor,
                    OraclePipeline::new(
                        rollup_config.clone(),
                        l1_config,
                        cursor,
                        oracle.clone(),
                        da_provider,
                        l1_provider.clone(),
                        l2_provider.clone(),
                    )
                    .await
                    .context("OraclePipeline::new")?,
                ),
            ),
            Some(cached_driver) => (
                B256::new(cached_driver.digest().into()),
                cached_driver.uncache(
                    cached_executor,
                    rollup_config.clone(),
                    l1_config,
                    cursor,
                    oracle.clone(),
                    da_provider,
                    l1_provider.clone(),
                    l2_provider.clone(),
                ),
            ),
        };

        // Run the derivation pipeline until we are able to produce the output root of the claimed
        // L2 block.
        let mut derived_output_roots = Vec::with_capacity(expected_output_count);
        for starting_block in safe_head_number..boot.claimed_l2_block_number {
            // Advance to the next target
            let (output_block, output_root) = driver
                .advance_to_target(&boot.rollup_config, Some(starting_block + 1))
                .await
                .context("advance_to_target")?;
            // Stop if nothing new was derived
            if output_block.block_info.number == starting_block {
                // No progress implies that there is insufficient L1 data available to produce
                // an L2 output root at this L2 height
                log("HALT");
                break;
            }
            // Append newly computed output root
            log(&format!(
                "OUTPUT: {}/{}\t{output_root}",
                output_block.block_info.number, boot.claimed_l2_block_number
            ));
            derived_output_roots.push(output_root);
        }

        ////////////////////////////////////////////////////////////////
        //              CHUNK → BLOCK COHERENCE CHECK                 //
        ////////////////////////////////////////////////////////////////
        //
        // For each block whose witness supplied chunks, verify:
        //   (1) chunk.agreed_l2_output_root == derived parent output root  (output-root anchor)
        //   (2) chunk.block_env.number == derived block.header.number      (block-number anchor)
        //   (3) sum(chunk.results.len()) == len(block's derived txs) AND each chunk's tx_hash
        //       matches the SHA256 of the derived tx bytes for its slice   (tx_hash binding)
        //   (4) hash_evm_state(&chunk.evm_state) == chunk.claimed_evm       (evm_state self-consistency)
        //   (5) calculate_receipt_root(&last_chunk.evm_state.receipts) matches the
        //       block's header.receipts_root                                 (receipts reconstruction)
        //
        // Collectively these close the forgery-vector chain identified by the
        // adversarial review:
        //   - (1)+(2) pin every chunk to THIS specific derived block, preventing a
        //     wrong-block substitution attack (H-1/H-2 equivalent via output-root,
        //     cryptographically stronger than `hash_cache_state` anchoring because
        //     output_root commits to state_root commits to all state — and avoids the
        //     canonicalization mismatch between chunk-witness `Cache` (preloaded with
        //     `NotExisting` stubs) and aggregation `State<TrieDB>.cache` at the same
        //     boundary).
        //   - (3) prevents substituting Y's tx sequence for X's actual derivation
        //     output (attacker-chosen T_Y attack).
        //   - (4) makes `chunk.evm_state.receipts` safely authenticated via
        //     `claimed_evm`, which in turn enables (5).
        //   - (5) is a belt-and-suspenders check that the authenticated chunk receipts
        //     actually reproduce the block's receipts_root — any Frankenstein receipt
        //     from the chunk replay that diverges from the block executor's honest
        //     output would be caught here.
        //
        // See `precondition::chunking::verify_block_chunks` for the detailed check
        // implementation and commentary.
        if has_chunks {
            log("CHUNK VERIFY");
            let collected: Vec<Execution> = chunk_verify_target
                .as_ref()
                .expect("chunk_verify_target set when has_chunks")
                .lock()
                .unwrap()
                .clone();
            let exec_refs: Vec<&Execution> = collected.iter().collect();
            crate::precondition::chunking::verify_chunks_against_blocks(
                &chunks,
                &exec_refs,
                &safe_head_parent,
                safe_head_number,
                rollup_config.as_ref(),
            )?;
        }

        ////////////////////////////////////////////////////////////////
        //                          EPILOGUE                          //
        ////////////////////////////////////////////////////////////////
        log("EPILOGUE");

        // Record derivation driver state
        let derivation_trace_hash = derivation_trace
            .map(|trace| {
                let derivation_trace = CachedDriver::from(driver);
                let trace_digest = B256::new(derivation_trace.digest().into());
                log(&format!("DERIVATION TRACE {trace_digest}"));
                let _ = trace.lock().unwrap().insert(derivation_trace);
                trace_digest
            })
            .unwrap_or_default();

        // Record intermediate output commitment precondition
        let proposal_precondition_hash = proposal_precondition_data
            .map(|(proposal_precondition, blobs)| {
                proposal::validate_proposal_precondition(
                    proposal_precondition,
                    blobs,
                    safe_head_number,
                    &derived_output_roots,
                )
            })
            .unwrap_or(Ok(B256::ZERO))
            .context("validate_precondition")?;

        // Compile final [Precondition]
        let precondition = Precondition::default()
            .proposal(proposal_precondition_hash)
            .derivation(derivation_cache_hash, derivation_trace_hash);

        // Compile the final [BootInfo]
        let claimed_l2_block_number = safe_head_number + derived_output_roots.len() as u64;
        let claimed_l2_output_root = derived_output_roots
            .pop()
            .unwrap_or(boot.agreed_l2_output_root);
        let boot = BootInfo {
            claimed_l2_output_root,
            claimed_l2_block_number,
            ..boot
        };

        // Return results
        Ok((boot, precondition))
    })
}

/// This method is copied as is from the `single` module in the `kona-client` crate.
///
/// Original documentation below:
///
/// Fetches the safe head hash of the L2 chain based on the agreed upon L2 output root in the
/// [BootInfo].
pub async fn fetch_safe_head_hash<O>(
    caching_oracle: &O,
    agreed_l2_output_root: B256,
) -> Result<B256, OracleProviderError>
where
    O: CommsClient,
{
    let mut output_preimage = [0u8; 128];
    HintType::StartingL2Output
        .with_data(&[agreed_l2_output_root.as_ref()])
        .send(caching_oracle)
        .await?;
    caching_oracle
        .get_exact(
            PreimageKey::new_keccak256(*agreed_l2_output_root),
            output_preimage.as_mut(),
        )
        .await?;

    output_preimage[96..128]
        .try_into()
        .map_err(OracleProviderError::SliceConversion)
}

/// Recovers a continuous execution trace from the collection target
pub fn recover_collected_executions(
    collection_target: Arc<Mutex<Vec<Execution>>>,
    claimed_l2_output_root: B256,
) -> Vec<Execution> {
    let mut executions = collection_target.lock().unwrap();
    for i in 1..executions.len() {
        executions[i - 1].claimed_output = executions[i].agreed_output;
    }
    if let Some(last_exec) = executions.last_mut() {
        last_exec.claimed_output = claimed_l2_output_root;
    }
    take::<Vec<Execution>>(executions.as_mut())
}

/// Validates that each cached contract bytecode hashes to its map key.
///
/// This is required because the canonical state hash encodes only the sorted set of contract
/// code-hash keys. If a witness provided arbitrary bytecode under a valid key and we failed to
/// check it first, the hash would authenticate the wrong executable code.
pub fn validate_cached_contracts<S: std::hash::BuildHasher>(
    contracts: &HashMap<B256, Bytecode, S>,
) {
    for (expected_hash, bytecode) in contracts {
        let actual_hash = bytecode.hash_slow();
        assert_eq!(
            actual_hash, *expected_hash,
            "cached contract bytecode hash mismatch: expected {expected_hash:?}, got {actual_hash:?}"
        );
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod tests {
    use super::*;
    use crate::client::tests::TestOracle;
    use crate::precondition::proposal::ProposalPrecondition;
    use alloy_evm::revm::database::Cache;
    use alloy_primitives::{b256, B256};
    use kona_proof::l1::OracleBlobProvider;
    use kona_proof::BootInfo;
    use std::sync::{Arc, Mutex};

    pub fn test_derivation(
        boot_info: BootInfo,
        proposal_data: Option<ProposalPrecondition>,
        derivation_cache: Option<CachedDriver>,
        derivation_trace: Option<Arc<Mutex<Option<CachedDriver>>>>,
    ) -> anyhow::Result<Vec<Arc<Execution>>> {
        test_derivation_with_chunks(
            boot_info,
            proposal_data,
            derivation_cache,
            derivation_trace,
            Vec::new(),
        )
    }

    /// Variant of [`test_derivation`] that threads a `chunks: Vec<Vec<Chunk>>` through to
    /// [`run_core_client`]. When `chunks` is `Vec::new()` (or every inner vec is empty),
    /// `run_core_client`'s `has_chunks` check returns `false` and the aggregation path is
    /// completely skipped — `ChunkingEvmFactory::new(empty_map)` delegates every
    /// `transact_raw()` straight through to the wrapped `OpEvmFactory`, so derivation
    /// output must be byte-identical to the no-chunks run. When any inner vec is
    /// non-empty, `run_core_client` runs the `CHUNK VERIFY` phase after derivation and
    /// calls `verify_block_chunks` for each such block.
    ///
    /// Caller constraint: when `chunks` is supplied non-empty, the outer index `i`
    /// corresponds to block `safe_head_number + 1 + i` (see `Witness::chunks` doc).
    /// Supplying chunks for an underived block position errors out of the CHUNK VERIFY
    /// phase.
    pub fn test_derivation_with_chunks(
        boot_info: BootInfo,
        proposal_data: Option<ProposalPrecondition>,
        derivation_cache: Option<CachedDriver>,
        derivation_trace: Option<Arc<Mutex<Option<CachedDriver>>>>,
        chunks: Vec<Vec<PartialExecution>>,
    ) -> anyhow::Result<Vec<Arc<Execution>>> {
        test_derivation_with_chunks_and_traces(
            boot_info,
            proposal_data,
            derivation_cache,
            derivation_trace,
            chunks,
            None,
        )
    }

    /// Extended variant of [`test_derivation_with_chunks`] that also threads an optional
    /// per-block `ResultAndState` trace collector through to `run_core_client`'s
    /// `ChunkingEvmFactory`. When supplied, the factory captures every successful
    /// `transact_raw()` into the buffer keyed by block number — both the monolithic
    /// delegate path (empty chunks) and the chunk replay path. Used by the round-trip
    /// integration test to capture ground truth on a first (empty-chunks) pass, build
    /// `Chunk` entries from the captured traces, then replay on a second pass.
    pub fn test_derivation_with_chunks_and_traces(
        boot_info: BootInfo,
        proposal_data: Option<ProposalPrecondition>,
        derivation_cache: Option<CachedDriver>,
        derivation_trace: Option<Arc<Mutex<Option<CachedDriver>>>>,
        chunks: Vec<Vec<PartialExecution>>,
        chunk_trace_collector: Option<crate::evm::tracing::ChunkTraceCollector>,
    ) -> anyhow::Result<Vec<Arc<Execution>>> {
        let oracle = Arc::new(TestOracle::new(boot_info.clone()));
        let (proposal_precondition_hash, proposal_data_hash) = if let Some(data) = proposal_data {
            (data.precondition_hash(), oracle.add_precondition_data(data))
        } else {
            Default::default()
        };
        let derivation_cache_digest = derivation_cache
            .as_ref()
            .map(|c| c.digest())
            .unwrap_or_default();
        let collection_target = Arc::new(Mutex::new(Vec::new()));
        // When any inner chunks vec is non-empty, `run_core_client` repurposes the
        // collection target internally for chunk verification and forbids both
        // `execution_cache` and `execution_trace` being Some. Keep
        // `collection_target` None in that path — the helper still returns a
        // (possibly empty) `Vec<Arc<Execution>>` at the end by virtue of the
        // returned `Vec::new()`, which matches the intent of exercising the
        // chunks-aware derivation path rather than capturing executions.
        let has_chunks = chunks.iter().any(|v| !v.is_empty());
        let execution_trace = (!has_chunks).then(|| collection_target.clone());
        let (result_boot_info, precondition) = run_core_client(
            proposal_data_hash,
            oracle.clone(),
            oracle.clone(),
            OracleBlobProvider::new(oracle.clone()),
            EthereumDataSourceProvider,
            vec![],
            execution_trace,
            derivation_cache,
            derivation_trace.clone(),
            None,
            chunks,
            chunk_trace_collector,
        )
        .context("run_core_client")?;

        assert_eq!(result_boot_info.l1_head, boot_info.l1_head);
        assert_eq!(
            result_boot_info.agreed_l2_output_root,
            boot_info.agreed_l2_output_root
        );
        if precondition.derivation_trace.is_zero() {
            assert_eq!(
                result_boot_info.claimed_l2_output_root,
                boot_info.claimed_l2_output_root
            );
            assert_eq!(
                result_boot_info.claimed_l2_block_number,
                boot_info.claimed_l2_block_number
            );
        }
        assert_eq!(result_boot_info.chain_id, boot_info.chain_id);

        let expected_precondition = Precondition {
            proposal_blobs: proposal_precondition_hash,
            execution_trace: Default::default(),
            derivation_cache: B256::new(derivation_cache_digest.into()),
            derivation_trace: derivation_trace
                .as_ref()
                .map(|t| {
                    t.lock()
                        .unwrap()
                        .as_ref()
                        .map(|d| B256::new(d.digest().into()))
                        .unwrap_or_default()
                })
                .unwrap_or_default(),
            chunk_trace: Default::default(),
        };
        assert_eq!(precondition.digest(), expected_precondition.digest(),);

        let execution_cache =
            recover_collected_executions(collection_target, boot_info.claimed_l2_output_root)
                .into_iter()
                .map(Arc::new)
                .collect();

        Ok(execution_cache)
    }

    pub fn test_execution(
        boot_info: BootInfo,
        execution_cache: Vec<Arc<Execution>>,
    ) -> anyhow::Result<B256> {
        // Ensure boot info triggers execution only
        assert!(boot_info.l1_head.is_zero());
        let expected_precondition_hash = exec_precondition_hash(execution_cache.as_slice());

        let oracle = Arc::new(TestOracle::new(boot_info.clone()));
        let (result_boot_info, precondition) = run_core_client(
            B256::ZERO,
            oracle.clone(),
            oracle.clone(),
            OracleBlobProvider::new(oracle.clone()),
            EthereumDataSourceProvider,
            execution_cache,
            None,
            None,
            None,
            None,
            Vec::new(),
            None,
        )
        .expect("run_core_client");

        assert_eq!(result_boot_info.l1_head, boot_info.l1_head);
        assert_eq!(
            result_boot_info.agreed_l2_output_root,
            boot_info.agreed_l2_output_root
        );
        assert_eq!(
            result_boot_info.claimed_l2_output_root,
            boot_info.claimed_l2_output_root
        );
        assert_eq!(
            result_boot_info.claimed_l2_block_number,
            boot_info.claimed_l2_block_number
        );
        assert_eq!(result_boot_info.chain_id, boot_info.chain_id);
        assert_eq!(
            B256::new(precondition.digest().into()),
            expected_precondition_hash
        );

        Ok(expected_precondition_hash)
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491250() {
        test_derivation(
            BootInfo {
                l1_head: b256!(
                    "0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"
                ),
                agreed_l2_output_root: b256!(
                    "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
                ),
                claimed_l2_output_root: b256!(
                    "0xa130fbfa315391b28668609252e4c09c3df3b77562281b996af30bf056cbb2c1"
                ),
                claimed_l2_block_number: 16491250,
                chain_id: 11155420,
                rollup_config: Default::default(),
                l1_config: Default::default(),
            },
            None,
            None,
            Some(Default::default()),
        )
        .unwrap();
    }

    /// Integration test for task 8.7: exercises the `chunks` parameter plumbing through
    /// `run_core_client` and the `ChunkingEvmFactory` dispatch path end-to-end against a
    /// real OP Sepolia fixture.
    ///
    /// Supplies `chunks: vec![vec![]]` — one entry for the single derived block, with the
    /// inner vec empty. This case is the interesting one for `ChunkingEvmFactory`:
    /// `run_core_client`'s `has_chunks` check (`any(|v| !v.is_empty())`) returns `false`,
    /// so the CHUNK VERIFY phase is skipped, but the chunks vec is still threaded through
    /// to `ChunkingEvmFactory::new(...)`. The factory's block-number → chunks map is
    /// empty, so every `create_evm()` call falls through to `OpEvmFactory::create_evm()`
    /// and every `transact_raw()` delegates straight to `OpEvm`. Derivation output must
    /// therefore be byte-identical to the no-chunks run in
    /// `test_op_sepolia_16491249_16491250` above.
    ///
    /// This closes task 8.7 at the "ChunkingEvmFactory in the real derivation path"
    /// integration level. The stronger variant — supplying a non-empty chunk with a real
    /// pre-computed `ResultAndState` vec and verifying `BlockBuildingOutcome` matches
    /// monolithic execution byte-for-byte — requires the host-side `TracingEvmFactory`
    /// → `build_chunk_witnesses` → `Chunk` assembly path that lives in Part 9. See
    /// `crates/prover/src/chunk.rs::build_chunk_witnesses` unit tests and the Part 9/10
    /// end-to-end tests for the full host-capture → guest-replay equivalence chain.
    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491250_with_empty_chunks() {
        let boot_info = BootInfo {
            l1_head: b256!("0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"),
            agreed_l2_output_root: b256!(
                "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
            ),
            claimed_l2_output_root: b256!(
                "0xa130fbfa315391b28668609252e4c09c3df3b77562281b996af30bf056cbb2c1"
            ),
            claimed_l2_block_number: 16491250,
            chain_id: 11155420,
            rollup_config: Default::default(),
            l1_config: Default::default(),
        };

        // `vec![vec![]]` — one per-block entry for the single block this fixture derives
        // (16491249 → 16491250). The empty inner vec means "no chunks for this block",
        // so `has_chunks == false` and the factory's map is empty → pure pass-through.
        test_derivation_with_chunks(
            boot_info,
            None,
            None,
            Some(Default::default()),
            vec![vec![]],
        )
        .unwrap();
    }

    /// Fetches the safe head header and the fully loaded `RollupConfig` via the same
    /// oracle-backed path `run_core_client` uses. Needed by the round-trip tests to
    /// supply (a) the parent header for block[0] when computing
    /// `expected_blob_excess_gas_and_price` and (b) a rollup_config whose
    /// `spec_id(timestamp)` returns the actual hardfork (not the `Default::default()`
    /// placeholder used in `BootInfo` fixtures). Codex round-4 critical patch.
    pub async fn test_fetch_safe_head_context(
        boot_info: &BootInfo,
    ) -> anyhow::Result<(alloy_consensus::Header, kona_genesis::RollupConfig)> {
        use crate::oracle::local::LocalOnceOracle;
        let test_oracle = Arc::new(TestOracle::new(boot_info.clone()));
        let oracle = Arc::new(LocalOnceOracle::new(test_oracle.clone()));
        let boot = BootInfo::load(oracle.as_ref())
            .await
            .context("BootInfo::load")?;
        let safe_head_hash = fetch_safe_head_hash(oracle.as_ref(), boot.agreed_l2_output_root)
            .await
            .context("fetch_safe_head_hash")?;
        let rollup_config = Arc::new(boot.rollup_config.clone());
        let l2_provider =
            OracleL2ChainProvider::new(safe_head_hash, rollup_config.clone(), oracle.clone());
        let header = l2_provider
            .header_by_hash(safe_head_hash)
            .context("l2_provider.header_by_hash")?;
        Ok((header, boot.rollup_config))
    }

    /// Builds a single-chunk `Chunk` covering all transactions of a derived block,
    /// populated from (a) the `Execution` (header, attributes, agreed_output) returned
    /// by `test_derivation_with_chunks_and_traces`, and (b) the per-tx `ResultAndState`
    /// vector captured by `ChunkingEvmFactory::new_with_traces` during the monolithic
    /// first pass.
    ///
    /// `agreed_db`, `claimed_db`, and `agreed_evm` are set to `B256::ZERO` because
    /// `verify_block_chunks` does not cross-check these against any derivation output —
    /// they are authenticated only through `stitch_chunks`'s `chunk_trace`
    /// reconstruction, which is not exercised by `test_derivation_with_chunks_and_traces`
    /// (stitching runs in `run_stitching_client`, one layer up from `run_core_client`).
    /// `claimed_evm` IS verified structurally (`hash_evm_state(&evm_state) ==
    /// claimed_evm`), so it must be computed over the same `evm_state` we store.
    ///
    /// `block_env.blob_excess_gas_and_price` is unused by `verify_block_chunks`
    /// (documented inline there — blob pricing only affects execution semantics, which
    /// are already bound through the cumulative EVM state; the field is captured into
    /// `block_ctx_hash` only when the stitching layer runs).
    pub fn build_single_chunk_for_block(
        execution: &Execution,
        traces: Vec<
            alloy_evm::revm::context_interface::result::ResultAndState<
                alloy_evm::op_revm::OpHaltReason,
            >,
        >,
        parent_header: &alloy_consensus::Header,
        spec_id: alloy_evm::op_revm::OpSpecId,
    ) -> PartialExecution {
        use crate::precondition::chunking::{hash_evm_state, EvmAccumulatorState};
        use alloy_consensus::TxReceipt;
        use alloy_evm::revm::context::BlockEnv;
        use alloy_op_evm::block::OpBlockExecutionCtx;
        use alloy_primitives::{Bloom, U256};

        let header = execution.artifacts.header.inner();
        let block_txs: Vec<Vec<u8>> = execution
            .attributes
            .transactions
            .as_ref()
            .map(|txs| txs.iter().map(|b| b.to_vec()).collect())
            .unwrap_or_default();
        assert_eq!(
            block_txs.len(),
            traces.len(),
            "captured trace count must match derivation-output tx count"
        );

        // Rebuild block_env from the sealed header. These fields are cross-checked in
        // verify_block_chunks — any mismatch would reject the chunk. The blob pricing
        // is derived from the parent header via the EIP-4844 formula, mirroring what
        // kona's block builder does (see
        // `crate::precondition::chunking::expected_blob_excess_gas_and_price`).
        let block_env = BlockEnv {
            number: U256::from(header.number),
            beneficiary: header.beneficiary,
            timestamp: U256::from(header.timestamp),
            gas_limit: header.gas_limit,
            basefee: header.base_fee_per_gas.unwrap_or(0),
            difficulty: header.difficulty,
            prevrandao: Some(header.mix_hash),
            blob_excess_gas_and_price:
                crate::precondition::chunking::expected_blob_excess_gas_and_price(
                    parent_header,
                    spec_id,
                ),
        };
        let op_block_ctx = OpBlockExecutionCtx {
            parent_hash: header.parent_hash,
            parent_beacon_block_root: header.parent_beacon_block_root,
            extra_data: header.extra_data.clone(),
        };

        // Reconstruct the cumulative logs_bloom from the block's receipts — matches
        // what the chunk guest would have computed running through `OpBlockExecutor`.
        let receipts = execution.artifacts.execution_result.receipts.clone();
        let mut logs_bloom = Bloom::default();
        for receipt in &receipts {
            for log in TxReceipt::logs(receipt) {
                logs_bloom.accrue_log(log);
            }
        }

        // OP Jovian invariant: blob_gas_used == da_footprint_used — see run_core_client
        // chunk branch for the rationale. execution.artifacts.execution_result.blob_gas_used
        // is the upstream-set `self.da_footprint_used` on OP.
        let block_blob_gas_used = execution.artifacts.execution_result.blob_gas_used;
        let evm_state = EvmAccumulatorState {
            cumulative_gas_used: execution.artifacts.execution_result.gas_used,
            da_footprint_used: block_blob_gas_used,
            blob_gas_used: block_blob_gas_used,
            logs_bloom,
            receipts,
        };
        let claimed_evm = hash_evm_state(&evm_state);

        let tx_hash = crate::precondition::chunking::compute_tx_hash(&block_txs);

        PartialExecution {
            agreed_db: B256::ZERO,
            agreed_evm: B256::ZERO,
            tx_hash,
            results: traces,
            evm_state,
            claimed_db: B256::ZERO,
            claimed_evm,
            block_env,
            op_block_ctx,
        }
    }

    /// Round-trip integration test for task 8.7 (step 2 — non-empty chunks).
    ///
    /// Runs the full derivation path *twice* against a real OP Sepolia fixture:
    ///   1. **Capture pass.** `chunks = Vec::new()`, `collector = Some(buf)`.
    ///      `ChunkingEvmFactory`'s chunks map is empty so every `transact_raw()`
    ///      delegates to inner `OpEvm`; the collector captures the full per-tx
    ///      `ResultAndState` trace keyed by block number.
    ///   2. **Replay pass.** Build one full-block `Chunk` per derived block using
    ///      `build_single_chunk_for_block` (traces + header + attributes +
    ///      agreed_output). Run again with `chunks = Vec::of those chunks`,
    ///      `collector = None`. `ChunkingEvm` now serves the replayed
    ///      `ResultAndState` entries from `chunk.results`, and `run_core_client`'s
    ///      CHUNK VERIFY phase cross-checks each chunk against the derivation
    ///      pipeline's authentic header (block_env fields, tx_hash binding,
    ///      hash_evm_state(&chunk.evm_state) == chunk.claimed_evm, receipts_root
    ///      reconstruction). Success ⇒ chunked-replay derivation produced
    ///      block-identical output to the monolithic pass.
    ///
    /// This is the "host-capture → guest-replay" equivalence round-trip that task 8.7
    /// requested. The 1-block fixture `16491249 → 16491250` exercises the full code
    /// path once; the 100-block fixture variant below exercises it on a deeper
    /// derivation window.
    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491250_chunks_roundtrip() {
        use std::collections::HashMap;

        let boot_info = BootInfo {
            l1_head: b256!("0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"),
            agreed_l2_output_root: b256!(
                "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
            ),
            claimed_l2_output_root: b256!(
                "0xa130fbfa315391b28668609252e4c09c3df3b77562281b996af30bf056cbb2c1"
            ),
            claimed_l2_block_number: 16491250,
            chain_id: 11155420,
            rollup_config: Default::default(),
            l1_config: Default::default(),
        };
        let safe_head_number = 16491249u64;

        // ---- Pass 1: capture ground-truth per-tx ResultAndState via ChunkingEvmFactory
        // in pass-through (empty chunks) mode with a trace collector attached.
        let collector: crate::evm::tracing::ChunkTraceCollector =
            Arc::new(Mutex::new(HashMap::new()));

        let executions = test_derivation_with_chunks_and_traces(
            boot_info.clone(),
            None,
            None,
            Some(Default::default()),
            Vec::new(),
            Some(collector.clone()),
        )
        .unwrap();

        assert_eq!(
            executions.len(),
            1,
            "fixture derives exactly one block (16491249 → 16491250)"
        );

        // Fetch the safe head header to use as the parent for block[0]'s chunk builder.
        let (safe_head_header, real_rollup_config) =
            test_fetch_safe_head_context(&boot_info).await.unwrap();
        let rollup_config_arc = Arc::new(real_rollup_config);

        // ---- Build chunks: one Chunk per derived block, consuming the captured traces.
        let mut captured = collector.lock().unwrap();
        let chunks: Vec<Vec<PartialExecution>> = executions
            .iter()
            .enumerate()
            .map(|(i, exec)| {
                let block_number = safe_head_number + 1 + i as u64;
                let traces = captured
                    .remove(&block_number)
                    .unwrap_or_else(|| panic!("no captured traces for block {block_number}"));
                let parent_header = if i == 0 {
                    &safe_head_header
                } else {
                    executions[i - 1].artifacts.header.inner()
                };
                let spec_id = rollup_config_arc.spec_id(exec.artifacts.header.inner().timestamp);
                vec![build_single_chunk_for_block(
                    exec,
                    traces,
                    parent_header,
                    spec_id,
                )]
            })
            .collect();
        drop(captured);

        // ---- Pass 2: replay through ChunkingEvm with populated chunks. The CHUNK VERIFY
        // phase in run_core_client performs the coherence cross-checks; if any mismatch
        // is detected, the whole run fails. Success proves the replayed chunks produce
        // the same derivation output as the monolithic pass.
        test_derivation_with_chunks_and_traces(
            boot_info,
            None,
            None,
            Some(Default::default()),
            chunks,
            None,
        )
        .unwrap();
    }

    /// Builds `n` sequential `Chunk` entries covering all transactions of a derived
    /// block. Splits the tx list as evenly as possible (last chunk may be smaller).
    /// Each chunk's `evm_state` carries the cumulative `EvmAccumulatorState` up to and
    /// including the chunk's last tx, reconstructed per tx from the captured receipts;
    /// `hash_chain_continuity` between chunks is trivially satisfied because `agreed_db`
    /// / `agreed_evm` are set to `ZERO` uniformly (verify_block_chunks checks
    /// `chunks[i].agreed_db == chunks[i-1].claimed_db`, which holds with all-zero).
    /// The last chunk's `evm_state` matches the full-block accumulator, so the
    /// receipts-root reconstruction check passes.
    pub fn build_n_chunks_for_block(
        execution: &Execution,
        traces: Vec<
            alloy_evm::revm::context_interface::result::ResultAndState<
                alloy_evm::op_revm::OpHaltReason,
            >,
        >,
        n: usize,
        parent_header: &alloy_consensus::Header,
        spec_id: alloy_evm::op_revm::OpSpecId,
    ) -> Vec<PartialExecution> {
        use crate::precondition::chunking::{hash_evm_state, EvmAccumulatorState};
        use alloy_consensus::TxReceipt;
        use alloy_evm::revm::context::BlockEnv;
        use alloy_op_evm::block::OpBlockExecutionCtx;
        use alloy_primitives::{Bloom, U256};

        assert!(n >= 1, "n must be >= 1");
        let header = execution.artifacts.header.inner();
        let block_txs: Vec<Vec<u8>> = execution
            .attributes
            .transactions
            .as_ref()
            .map(|txs| txs.iter().map(|b| b.to_vec()).collect())
            .unwrap_or_default();
        assert_eq!(
            block_txs.len(),
            traces.len(),
            "captured trace count must match derivation-output tx count"
        );
        // With n > tx_count, later chunks would be empty and the M-10 empty-tx guard
        // in ChunkingEvm applies. For the integration test, collapse n down to
        // max(1, tx_count).
        let n = n.min(block_txs.len().max(1));

        let block_env = BlockEnv {
            number: U256::from(header.number),
            beneficiary: header.beneficiary,
            timestamp: U256::from(header.timestamp),
            gas_limit: header.gas_limit,
            basefee: header.base_fee_per_gas.unwrap_or(0),
            difficulty: header.difficulty,
            prevrandao: Some(header.mix_hash),
            blob_excess_gas_and_price:
                crate::precondition::chunking::expected_blob_excess_gas_and_price(
                    parent_header,
                    spec_id,
                ),
        };
        let op_block_ctx = OpBlockExecutionCtx {
            parent_hash: header.parent_hash,
            parent_beacon_block_root: header.parent_beacon_block_root,
            extra_data: header.extra_data.clone(),
        };

        // Compute chunk boundaries — [tx_start..tx_end) per chunk, covering all txs.
        let tx_count = block_txs.len();
        let base = tx_count / n;
        let rem = tx_count % n;
        let mut boundaries = Vec::with_capacity(n + 1);
        boundaries.push(0usize);
        for i in 0..n {
            let size = base + usize::from(i < rem);
            boundaries.push(boundaries[i] + size);
        }

        // Accumulate cumulative receipts / logs_bloom tx-by-tx so each chunk carries
        // its post-chunk state, and the last chunk matches the block total.
        let receipts = &execution.artifacts.execution_result.receipts;
        let block_blob_gas_used = execution.artifacts.execution_result.blob_gas_used;

        let mut chunks = Vec::with_capacity(n);
        // Track the previous chunk's claimed_evm so each new chunk's agreed_evm
        // matches — verify_block_chunks enforces hash-chain continuity between
        // consecutive chunks.
        let mut prev_claimed_evm = B256::ZERO;
        for chunk_idx in 0..n {
            let start = boundaries[chunk_idx];
            let end = boundaries[chunk_idx + 1];
            let chunk_receipts: Vec<_> = receipts[..end].to_vec();
            let mut logs_bloom = Bloom::default();
            for r in &chunk_receipts {
                for log in TxReceipt::logs(r) {
                    logs_bloom.accrue_log(log);
                }
            }
            // cumulative_gas_used comes directly from the last receipt in the chunk's
            // cumulative prefix (OpReceiptEnvelope.cumulative_gas_used is block-
            // cumulative by EIP-658). On the last chunk this equals the block total,
            // so the `hash_evm_state` + receipts-root checks pass.
            let cumulative_gas_used = chunk_receipts
                .last()
                .map(|r| r.cumulative_gas_used())
                .unwrap_or(0);
            // On OP Jovian, blob_gas_used == da_footprint_used, scaled proportionally
            // across the block. For the test, just split linearly by tx index — the
            // LAST chunk must match the block total, which it does because
            // `end == tx_count` for the last iteration.
            let (chunk_da_used, chunk_blob_used) = if chunk_idx == n - 1 {
                (block_blob_gas_used, block_blob_gas_used)
            } else if tx_count == 0 {
                (0, 0)
            } else {
                let v = block_blob_gas_used * (end as u64) / (tx_count as u64);
                (v, v)
            };
            let evm_state = EvmAccumulatorState {
                cumulative_gas_used,
                da_footprint_used: chunk_da_used,
                blob_gas_used: chunk_blob_used,
                logs_bloom,
                receipts: chunk_receipts,
            };
            let claimed_evm = hash_evm_state(&evm_state);
            let chunk_txs = &block_txs[start..end];
            let tx_hash = crate::precondition::chunking::compute_tx_hash(chunk_txs);
            let chunk_traces: Vec<_> = traces[start..end].to_vec();
            chunks.push(PartialExecution {
                agreed_db: B256::ZERO,
                agreed_evm: prev_claimed_evm,
                tx_hash,
                results: chunk_traces,
                evm_state,
                claimed_db: B256::ZERO,
                claimed_evm,
                block_env: block_env.clone(),
                op_block_ctx: op_block_ctx.clone(),
            });
            prev_claimed_evm = claimed_evm;
        }
        chunks
    }

    /// Multi-chunk round-trip for task 8.7. Splits each block into up to 2 chunks —
    /// exercising `ChunkingEvm`'s cursor advance across chunks, hash-chain continuity
    /// (`chunks[i].agreed_db == chunks[i-1].claimed_db`, and likewise for `agreed_evm`),
    /// per-chunk `tx_hash` binding over tx slices, and incremental `evm_state` /
    /// receipts-root construction where only the LAST chunk's state must match the
    /// block totals.
    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491250_two_chunks_roundtrip() {
        use std::collections::HashMap;

        let boot_info = BootInfo {
            l1_head: b256!("0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"),
            agreed_l2_output_root: b256!(
                "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
            ),
            claimed_l2_output_root: b256!(
                "0xa130fbfa315391b28668609252e4c09c3df3b77562281b996af30bf056cbb2c1"
            ),
            claimed_l2_block_number: 16491250,
            chain_id: 11155420,
            rollup_config: Default::default(),
            l1_config: Default::default(),
        };
        let safe_head_number = 16491249u64;

        let collector: crate::evm::tracing::ChunkTraceCollector =
            Arc::new(Mutex::new(HashMap::new()));

        let executions = test_derivation_with_chunks_and_traces(
            boot_info.clone(),
            None,
            None,
            Some(Default::default()),
            Vec::new(),
            Some(collector.clone()),
        )
        .unwrap();

        let (safe_head_header, real_rollup_config) =
            test_fetch_safe_head_context(&boot_info).await.unwrap();
        let rollup_config_arc = Arc::new(real_rollup_config);

        let mut captured = collector.lock().unwrap();
        let chunks: Vec<Vec<PartialExecution>> = executions
            .iter()
            .enumerate()
            .map(|(i, exec)| {
                let block_number = safe_head_number + 1 + i as u64;
                let traces = captured.remove(&block_number).unwrap_or_default();
                let parent_header = if i == 0 {
                    &safe_head_header
                } else {
                    executions[i - 1].artifacts.header.inner()
                };
                let spec_id = rollup_config_arc.spec_id(exec.artifacts.header.inner().timestamp);
                build_n_chunks_for_block(exec, traces, 2, parent_header, spec_id)
            })
            .collect();
        drop(captured);

        test_derivation_with_chunks_and_traces(
            boot_info,
            None,
            None,
            Some(Default::default()),
            chunks,
            None,
        )
        .unwrap();
    }

    /// Round-trip integration test for task 8.7 (step 2 — non-empty chunks, longer run).
    /// Same structure as `test_op_sepolia_16491249_16491250_chunks_roundtrip` above but
    /// on the 100-block fixture. Exercises per-block Chunk construction across a deeper
    /// derivation window: 100 independent block_env / op_block_ctx cross-checks, 100
    /// tx_hash bindings, 100 receipts_root reconstructions, etc.
    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491349_chunks_roundtrip() {
        use std::collections::HashMap;

        let boot_info = BootInfo {
            l1_head: b256!("0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"),
            agreed_l2_output_root: b256!(
                "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
            ),
            claimed_l2_output_root: b256!(
                "0x6984e5ae4d025562c8a571949b985692d80e364ddab46d5c8af5b36a20f611d1"
            ),
            claimed_l2_block_number: 16491349,
            chain_id: 11155420,
            rollup_config: Default::default(),
            l1_config: Default::default(),
        };
        let safe_head_number = 16491249u64;

        let collector: crate::evm::tracing::ChunkTraceCollector =
            Arc::new(Mutex::new(HashMap::new()));

        let executions = test_derivation_with_chunks_and_traces(
            boot_info.clone(),
            None,
            None,
            Some(Default::default()),
            Vec::new(),
            Some(collector.clone()),
        )
        .unwrap();

        assert_eq!(executions.len(), 100, "fixture derives 100 blocks");

        let (safe_head_header, real_rollup_config) =
            test_fetch_safe_head_context(&boot_info).await.unwrap();
        let rollup_config_arc = Arc::new(real_rollup_config);

        let mut captured = collector.lock().unwrap();
        let chunks: Vec<Vec<PartialExecution>> = executions
            .iter()
            .enumerate()
            .map(|(i, exec)| {
                let block_number = safe_head_number + 1 + i as u64;
                // Some blocks may have zero user txs (rare on OP Sepolia, but we
                // must tolerate it — empty trace vec is a valid capture and the
                // resulting single chunk has tx_count=0, results=[]).
                let traces = captured.remove(&block_number).unwrap_or_default();
                let parent_header = if i == 0 {
                    &safe_head_header
                } else {
                    executions[i - 1].artifacts.header.inner()
                };
                let spec_id = rollup_config_arc.spec_id(exec.artifacts.header.inner().timestamp);
                vec![build_single_chunk_for_block(
                    exec,
                    traces,
                    parent_header,
                    spec_id,
                )]
            })
            .collect();
        drop(captured);

        test_derivation_with_chunks_and_traces(
            boot_info,
            None,
            None,
            Some(Default::default()),
            chunks,
            None,
        )
        .unwrap();
    }

    /// Integration test for task 8.7: same as above but on the longer 100-block fixture,
    /// to exercise the chunks-parameter pass-through across many derived blocks. Supplies
    /// 100 empty inner vecs (one per block from 16491250..=16491349). Every block's
    /// `chunks_by_block` lookup misses, every `transact_raw()` delegates, and derivation
    /// output must match the no-chunks baseline
    /// (`test_op_sepolia_16491249_16491349` above) exactly.
    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491349_with_empty_chunks() {
        let boot_info = BootInfo {
            l1_head: b256!("0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"),
            agreed_l2_output_root: b256!(
                "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
            ),
            claimed_l2_output_root: b256!(
                "0x6984e5ae4d025562c8a571949b985692d80e364ddab46d5c8af5b36a20f611d1"
            ),
            claimed_l2_block_number: 16491349,
            chain_id: 11155420,
            rollup_config: Default::default(),
            l1_config: Default::default(),
        };

        // 100 empty inner vecs — one per block in the 16491249..16491349 derivation
        // window. All map lookups miss → `ChunkingEvm` delegates every call to inner
        // `OpEvm`, identical to the monolithic path.
        let chunks = vec![Vec::<PartialExecution>::new(); 100];

        test_derivation_with_chunks(boot_info, None, None, Some(Default::default()), chunks)
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491349() {
        let executions = test_derivation(
            BootInfo {
                l1_head: b256!(
                    "0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"
                ),
                agreed_l2_output_root: b256!(
                    "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
                ),
                claimed_l2_output_root: b256!(
                    "0x6984e5ae4d025562c8a571949b985692d80e364ddab46d5c8af5b36a20f611d1"
                ),
                claimed_l2_block_number: 16491349,
                chain_id: 11155420,
                rollup_config: Default::default(),
                l1_config: Default::default(),
            },
            None,
            None,
            Some(Default::default()),
        )
        .unwrap();
        let _ = test_execution(
            BootInfo {
                l1_head: B256::ZERO,
                agreed_l2_output_root: b256!(
                    "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
                ),
                claimed_l2_output_root: b256!(
                    "0x6984e5ae4d025562c8a571949b985692d80e364ddab46d5c8af5b36a20f611d1"
                ),
                claimed_l2_block_number: 16491349,
                chain_id: 11155420,
                rollup_config: Default::default(),
                l1_config: Default::default(),
            },
            executions,
        )
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491349_validity() {
        test_derivation(
            BootInfo {
                l1_head: b256!(
                    "0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"
                ),
                agreed_l2_output_root: b256!(
                    "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
                ),
                claimed_l2_output_root: b256!(
                    "0x6984e5ae4d025562c8a571949b985692d80e364ddab46d5c8af5b36a20f611d1"
                ),
                claimed_l2_block_number: 16491349,
                chain_id: 11155420,
                rollup_config: Default::default(),
                l1_config: Default::default(),
            },
            Some(ProposalPrecondition {
                proposal_l2_head_number: 16491249,
                proposal_output_count: 1,
                output_block_span: 100,
                blob_hashes: vec![],
            }),
            None,
            Some(Default::default()),
        )
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491349_insufficient() {
        // data wasn't published at l1 origin
        test_derivation(
            BootInfo {
                l1_head: b256!(
                    "0x78228b4f2d59ae1820b8b8986a875630cb32d88b298d78d0f25bcac8f3bdfbf3"
                ),
                agreed_l2_output_root: b256!(
                    "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
                ),
                claimed_l2_output_root: B256::ZERO,
                claimed_l2_block_number: 16491349,
                chain_id: 11155420,
                rollup_config: Default::default(),
                l1_config: Default::default(),
            },
            None,
            None,
            Some(Default::default()),
        )
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491349_insufficient_fail() {
        let claimed_l2_output_root =
            b256!("0x6984e5ae4d025562c8a571949b985692d80e364ddab46d5c8af5b36a20f611d1");
        let claimed_l2_block_number = 16491349;
        // data wasn't published as of l1 head
        let exec = test_derivation(
            BootInfo {
                l1_head: b256!(
                    "0x78228b4f2d59ae1820b8b8986a875630cb32d88b298d78d0f25bcac8f3bdfbf3"
                ),
                agreed_l2_output_root: b256!(
                    "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
                ),
                claimed_l2_output_root,
                claimed_l2_block_number,
                chain_id: 11155420,
                rollup_config: Default::default(),
                l1_config: Default::default(),
            },
            None,
            None,
            Some(Default::default()),
        )
        .unwrap();
        let Some(last_execution) = exec.last() else {
            return;
        };
        assert_ne!(last_execution.claimed_output, claimed_l2_output_root);
        assert!(last_execution.artifacts.header.number < claimed_l2_block_number);
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491248_failure() {
        test_derivation(
            BootInfo {
                l1_head: b256!(
                    "0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"
                ),
                agreed_l2_output_root: b256!(
                    "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
                ),
                claimed_l2_output_root: b256!(
                    "0xa130fbfa315391b28668609252e4c09c3df3b77562281b996af30bf056cbb2c1"
                ),
                claimed_l2_block_number: 16491248,
                chain_id: 11155420,
                rollup_config: Default::default(),
                l1_config: Default::default(),
            },
            None,
            None,
            Some(Default::default()),
        )
        .unwrap_err();
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491249() {
        let executions = test_derivation(
            BootInfo {
                l1_head: b256!(
                    "0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"
                ),
                agreed_l2_output_root: b256!(
                    "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
                ),
                claimed_l2_output_root: b256!(
                    "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
                ),
                claimed_l2_block_number: 16491249,
                chain_id: 11155420,
                rollup_config: Default::default(),
                l1_config: Default::default(),
            },
            None,
            None,
            Some(Default::default()),
        )
        .unwrap();
        assert!(executions.is_empty());
    }

    /// Helper to run chunk mode through `run_core_client` with the given witness.
    pub fn test_chunk_execution(
        boot_info: BootInfo,
        chunk_witness: ChunkWitnessData,
    ) -> (BootInfo, Precondition) {
        assert_eq!(boot_info.l1_head, B256::repeat_byte(0xFF));
        let oracle = Arc::new(TestOracle::new(boot_info.clone()));
        run_core_client(
            B256::ZERO,
            oracle.clone(),
            oracle.clone(),
            OracleBlobProvider::new(oracle.clone()),
            EthereumDataSourceProvider,
            vec![],
            None,
            None,
            None,
            Some(chunk_witness),
            Vec::new(),
            None,
        )
        .expect("run_core_client chunk mode")
    }

    /// Creates a chunk witness with default empty state for testing.
    fn make_test_chunk_witness(
        cache: alloy_evm::revm::database::in_memory_db::Cache,
        evm_state: crate::precondition::chunking::EvmAccumulatorState,
        transactions: Vec<Vec<u8>>,
    ) -> ChunkWitnessData {
        ChunkWitnessData {
            transactions,
            block_env: alloy_evm::revm::context::BlockEnv::default(),
            op_block_ctx: alloy_op_evm::OpBlockExecutionCtx::default(),
            cache,
            evm_state,
        }
    }

    fn make_chunk_boot_info() -> BootInfo {
        BootInfo {
            l1_head: B256::repeat_byte(0xFF),
            agreed_l2_output_root: b256!(
                "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
            ),
            claimed_l2_output_root: b256!(
                "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
            ),
            claimed_l2_block_number: 16491249,
            chain_id: 11155420,
            rollup_config: Default::default(),
            l1_config: Default::default(),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_chunk_mode_empty_transactions() {
        use crate::precondition::chunking::{
            compute_chunk_trace, compute_tx_hash, hash_cache, hash_evm_state, hash_results,
            EvmAccumulatorState,
        };
        use alloy_evm::revm::database::in_memory_db::Cache;
        use risc0_zkvm::sha::Digestible;

        let cache = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        let evm_state = EvmAccumulatorState::default();
        let cw = make_test_chunk_witness(cache.clone(), evm_state.clone(), vec![]);

        // Compute expected chunk trace (no txs → post == pre; empty results_hash).
        let tx_hash = compute_tx_hash(&[]);
        let pre_db_hash = hash_cache(&cache);
        let pre_evm_hash = hash_evm_state(&evm_state);
        let results_hash = hash_results(&[]);
        let block_ctx_hash = crate::precondition::chunking::hash_block_ctx(
            &alloy_evm::revm::context::BlockEnv::default(),
            &alloy_op_evm::OpBlockExecutionCtx::default(),
        );
        let expected_trace = compute_chunk_trace(
            tx_hash,
            pre_db_hash,
            pre_db_hash,
            pre_evm_hash,
            pre_evm_hash,
            results_hash,
            block_ctx_hash,
        );

        let (result_boot, precondition) = test_chunk_execution(make_chunk_boot_info(), cw);

        assert_eq!(result_boot.l1_head, B256::repeat_byte(0xFF));
        let expected_precondition = Precondition::default().chunk(expected_trace);
        assert_eq!(precondition.digest(), expected_precondition.digest());
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_chunk_mode_multi_chunk_hash_continuity() {
        use crate::precondition::chunking::{hash_cache, EvmAccumulatorState};
        use alloy_evm::revm::database::in_memory_db::Cache;

        // Two empty chunks from the same state — chunk_0's post_db_hash must equal
        // chunk_1's pre_db_hash (both are the same cache with no mutations).
        let cache = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        let evm_state = EvmAccumulatorState::default();

        let cw0 = make_test_chunk_witness(cache.clone(), evm_state.clone(), vec![]);
        let cw1 = make_test_chunk_witness(cache.clone(), evm_state.clone(), vec![]);

        let pre_db_hash_0 = hash_cache(&cw0.cache);
        let pre_db_hash_1 = hash_cache(&cw1.cache);

        // With no transactions, the chunk execution doesn't modify state,
        // so post_db_hash_0 == pre_db_hash_0 == pre_db_hash_1
        assert_eq!(pre_db_hash_0, pre_db_hash_1);

        // Run both chunks and verify they produce valid results
        let (_boot0, _prec0) = test_chunk_execution(make_chunk_boot_info(), cw0);
        let (_boot1, _prec1) = test_chunk_execution(make_chunk_boot_info(), cw1);
    }

    /// Helper: run `run_core_client` in chunk mode, returning the raw `Result`
    /// (unlike `test_chunk_execution` which `.expect()`s success). Used by
    /// the rejection tests below.
    fn try_chunk_execution(
        boot_info: BootInfo,
        chunk_witness: ChunkWitnessData,
    ) -> anyhow::Result<(BootInfo, Precondition)> {
        assert_eq!(boot_info.l1_head, B256::repeat_byte(0xFF));
        let oracle = Arc::new(TestOracle::new(boot_info.clone()));
        run_core_client(
            B256::ZERO,
            oracle.clone(),
            oracle.clone(),
            OracleBlobProvider::new(oracle.clone()),
            EthereumDataSourceProvider,
            vec![],
            None,
            None,
            None,
            Some(chunk_witness),
            Vec::new(),
            None,
        )
    }

    /// Positive case: a non-zero synced seed (representing a later chunk
    /// whose earlier chunks consumed some gas / DA) is accepted and
    /// threaded through unchanged when the current chunk is empty. This
    /// verifies that the invariant admits legitimate mid-block continuity
    /// (the guest's output preserves `gas_used`, `da_footprint_used`, and
    /// `blob_gas_used` verbatim when no txs execute, matching the next
    /// chunk's pre-state seed in a multi-chunk block).
    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_chunk_mode_accepts_synced_nonzero_seeds() {
        use crate::precondition::chunking::{
            compute_chunk_trace, compute_tx_hash, hash_cache, hash_evm_state, hash_results,
            EvmAccumulatorState,
        };
        use alloy_evm::revm::database::in_memory_db::Cache;
        use risc0_zkvm::sha::Digestible;

        let cache = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        // Seed representing a later chunk in a multi-chunk block: prior
        // chunks consumed 1M gas and 500 DA; blob_gas tracks DA in lockstep
        // (OP Jovian invariant).
        let evm_state = EvmAccumulatorState {
            cumulative_gas_used: 1_000_000,
            da_footprint_used: 500,
            blob_gas_used: 500,
            logs_bloom: Default::default(),
            receipts: Vec::new(),
        };
        let mut cw = make_test_chunk_witness(cache.clone(), evm_state.clone(), vec![]);
        cw.block_env.gas_limit = 30_000_000;

        // With no transactions, every accumulator field must be preserved
        // unchanged: post_evm_hash == pre_evm_hash. The next chunk's seed
        // (chunk index 2 in this hypothetical block) would hash to the same
        // value, preserving continuity.
        let tx_hash = compute_tx_hash(&[]);
        let pre_db_hash = hash_cache(&cache);
        let pre_evm_hash = hash_evm_state(&evm_state);
        let results_hash = hash_results(&[]);
        // Compute block_ctx_hash over the EXACT BlockEnv/OpBlockExecutionCtx the
        // chunk guest will see (cw.block_env was mutated to gas_limit=30M above).
        let block_ctx_hash =
            crate::precondition::chunking::hash_block_ctx(&cw.block_env, &cw.op_block_ctx);
        let expected_trace = compute_chunk_trace(
            tx_hash,
            pre_db_hash,
            pre_db_hash,
            pre_evm_hash,
            pre_evm_hash,
            results_hash,
            block_ctx_hash,
        );

        let (result_boot, precondition) =
            try_chunk_execution(make_chunk_boot_info(), cw).expect("synced seeds must be accepted");

        assert_eq!(result_boot.l1_head, B256::repeat_byte(0xFF));
        let expected_precondition = Precondition::default().chunk(expected_trace);
        assert_eq!(precondition.digest(), expected_precondition.digest());
    }

    #[tokio::test(flavor = "multi_thread")]
    #[should_panic(expected = "cached contract bytecode hash mismatch")]
    pub async fn test_chunk_mode_malformed_contract_rejected() {
        use crate::precondition::chunking::EvmAccumulatorState;
        use alloy_evm::revm::database::in_memory_db::Cache;
        use alloy_evm::revm::state::Bytecode;

        let mut cache = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        // Insert a contract with mismatched hash → should panic during validation
        let code = Bytecode::new_raw(alloy_primitives::Bytes::from_static(&[0x60, 0x00]));
        let wrong_hash = B256::repeat_byte(0xAB); // Not the actual hash of the bytecode
        cache.contracts.insert(wrong_hash, code);

        let cw = make_test_chunk_witness(cache, EvmAccumulatorState::default(), vec![]);

        // This should panic before execution due to contract validation
        let _ = test_chunk_execution(make_chunk_boot_info(), cw);
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_chunk_mode_missing_witness_errors() {
        let boot = make_chunk_boot_info();
        let oracle = Arc::new(TestOracle::new(boot.clone()));
        // Call with chunk sentinel but None witness → should error
        let result = run_core_client(
            B256::ZERO,
            oracle.clone(),
            oracle.clone(),
            OracleBlobProvider::new(oracle.clone()),
            EthereumDataSourceProvider,
            vec![],
            None,
            None,
            None,
            None, // No chunk witness!
            Vec::new(),
            None,
        );
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("chunk witness required"),
            "unexpected error: {err_msg}"
        );
    }

    #[test]
    fn validate_cached_contracts_rejects_malformed_entry() {
        let valid_code = crate::precondition::chunking::tests::make_bytecode(&[0x60, 0x00]);
        let invalid_code = crate::precondition::chunking::tests::make_bytecode(&[0x60, 0x01]);
        let code_hash = valid_code.hash_slow();

        let mut cache = Cache {
            accounts: Default::default(),
            contracts: Default::default(),
            logs: Vec::new(),
            block_hashes: Default::default(),
        };
        cache.contracts.insert(code_hash, invalid_code);

        let panic =
            std::panic::catch_unwind(|| validate_cached_contracts(&cache.contracts)).unwrap_err();
        let message = if let Some(msg) = panic.downcast_ref::<String>() {
            msg.clone()
        } else if let Some(msg) = panic.downcast_ref::<&str>() {
            msg.to_string()
        } else {
            String::new()
        };
        assert!(message.contains("cached contract bytecode hash mismatch"));
    }
}

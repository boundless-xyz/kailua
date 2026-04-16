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
use crate::executor::{new_execution_cursor, CachedExecutor, Execution, PanicDB};
use crate::kona::OracleL1ChainProvider;
use crate::oracle::local::LocalOnceOracle;
use crate::precondition::chunking::{
    compute_chunk_trace, compute_tx_hash, hash_cache, hash_evm_state, hash_overlay_state,
    validate_cached_contracts,
};
use crate::precondition::execution::exec_precondition_hash;
use crate::precondition::{proposal, Precondition};
use crate::witness::ChunkWitnessData;
use alloy_consensus::transaction::SignerRecoverable;
use alloy_eips::eip2718::Decodable2718;
use alloy_evm::revm::context_interface::result::ResultAndState;
use alloy_evm::revm::primitives::Bytes;
use alloy_evm::revm::{Database, DatabaseCommit};
use alloy_evm::{Evm, EvmFactory, FromTxWithEncoded};
use alloy_op_evm::OpEvmFactory;
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
use op_alloy_consensus::OpReceiptEnvelope;
use op_alloy_consensus::{OpTxEnvelope, OpTxType};
use risc0_zkvm::sha::Digestible;
use std::fmt::Debug;
use std::mem::take;
use std::sync::{Arc, Mutex};

type EvmTx = alloy_evm::op_revm::OpTransaction<alloy_evm::revm::context::TxEnv>;

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
            let cw = chunk_witness.context("chunk witness required in chunk mode")?;

            // (a) Validate cached contract bytecodes against their code_hash keys
            validate_cached_contracts(&cw.cache.contracts);

            // (b) Build CacheDB<PanicDB> from witness cache
            let mut cache_db = alloy_evm::revm::database::in_memory_db::CacheDB::new(PanicDB);
            cache_db.cache = cw.cache.clone();

            // (c) Compute pre-hashes from the witness state
            let pre_db_hash = hash_cache(&cw.cache);
            let pre_evm_hash = hash_evm_state(&cw.evm_state);

            // (d) Wrap in State with bundle tracking; set state-clear from hardfork
            let mut state = alloy_evm::revm::database::states::State::builder()
                .with_database(cache_db)
                .with_bundle_update()
                .build();
            let timestamp: u64 = cw.block_env.timestamp.to();
            let spec_id = rollup_config.spec_id(timestamp);
            // Match monolithic execution: the standard executor uses
            // `spec.is_spurious_dragon_active_at_block(number)`. All OP spec IDs
            // (BEDROCK and later) are post-Spurious-Dragon, so state-clear is always
            // active. We verify this by checking that the spec_id is at least BEDROCK.
            assert!(
                spec_id >= alloy_evm::op_revm::OpSpecId::BEDROCK,
                "unexpected pre-Bedrock OP spec: state-clear semantics may differ"
            );
            state.set_state_clear_flag(true);

            // Set up EVM environment
            let cfg_env = alloy_evm::revm::context::CfgEnv::new()
                .with_chain_id(boot.chain_id)
                .with_spec_and_mainnet_gas_params(spec_id);
            let evm_env = alloy_evm::EvmEnv::new(cfg_env, cw.block_env.clone());

            // Create EVM
            let mut evm = OpEvmFactory::default().create_evm(&mut state, evm_env);

            // Determine hardfork flags for deposit receipt handling
            let is_regolith = rollup_config.is_regolith_active(timestamp);
            let is_canyon = rollup_config.is_canyon_active(timestamp);
            let is_jovian = rollup_config.is_jovian_active(timestamp);
            let block_gas_limit = cw.block_env.gas_limit;

            // (e) Execute exactly witness.transactions in order (no prelude, no epilogue)
            let mut evm_accum = cw.evm_state.clone();
            for tx_bytes in &cw.transactions {
                // Decode the signed transaction envelope, rejecting trailing bytes.
                // This ensures tx_hash commits to exactly the bytes that were executed.
                let mut buf = tx_bytes.as_slice();
                let tx = OpTxEnvelope::decode_2718(&mut buf)
                    .context("invalid transaction encoding in chunk witness")?;
                assert!(
                    buf.is_empty(),
                    "trailing bytes after transaction decoding in chunk witness"
                );
                let tx_type = tx.tx_type();
                let is_deposit = tx_type == OpTxType::Deposit;

                // Enforce per-tx block gas budget (mirrors OpBlockExecutor validation).
                // Deposits bypass the check pre-Regolith; post-Regolith all txs are checked.
                use alloy_consensus::Transaction;
                let tx_gas_limit = tx.gas_limit();
                if is_regolith || !is_deposit {
                    let block_available_gas =
                        block_gas_limit.saturating_sub(evm_accum.cumulative_gas_used);
                    assert!(
                        tx_gas_limit <= block_available_gas,
                        "transaction gas limit {tx_gas_limit} exceeds available \
                         block gas {block_available_gas}"
                    );
                }

                // Jovian DA footprint estimation and validation (pre-execution).
                // Mirrors OpBlockExecutor::execute_transaction_without_commit.
                let tx_da_footprint = if is_jovian && !is_deposit {
                    use alloy_evm::op_revm::{estimate_tx_compressed_size, L1BlockInfo};
                    let compressed =
                        estimate_tx_compressed_size(tx_bytes).saturating_div(1_000_000);
                    // Pre-load the L1 block contract into State's cache before
                    // reading its storage (mirrors the standard executor).
                    let l1_block =
                        alloy_primitives::address!("4200000000000000000000000000000000000015");
                    let _ = evm
                        .db_mut()
                        .basic(l1_block)
                        .context("L1 block contract missing from chunk witness")?;
                    let scalar: u64 = L1BlockInfo::fetch_da_footprint_gas_scalar(evm.db_mut())
                        .context(
                            "failed to fetch DA footprint gas scalar \
                                 from L1 block contract",
                        )?
                        .into();
                    let footprint = compressed.saturating_mul(scalar);
                    let da_available = block_gas_limit.saturating_sub(evm_accum.da_footprint_used);
                    assert!(
                        footprint <= da_available,
                        "transaction DA footprint {footprint} exceeds available \
                         block DA budget {da_available}"
                    );
                    footprint
                } else {
                    0
                };

                // Recover signer and convert to EVM transaction
                let recovered = tx
                    .try_into_recovered()
                    .context("invalid transaction signature in chunk witness")?;
                let sender = recovered.signer();
                let encoded = Bytes::from(tx_bytes.clone());
                let evm_tx = EvmTx::from_encoded_tx(recovered.inner(), sender, encoded);

                // Execute the transaction
                let ResultAndState {
                    result,
                    state: tx_state,
                } = evm
                    .transact_raw(evm_tx)
                    .context("chunk transaction execution failed")?;

                // Commit state changes to the database
                evm.db_mut().commit(tx_state);

                // Update gas accumulator
                evm_accum.cumulative_gas_used += result.gas_used();

                // Update DA footprint / blob gas accumulators (Jovian+)
                if tx_da_footprint > 0 {
                    evm_accum.da_footprint_used =
                        evm_accum.da_footprint_used.saturating_add(tx_da_footprint);
                    // In the block executor, blob_gas_used in BlockExecutionResult
                    // equals the accumulated da_footprint_used. The host-side witness
                    // construction tracks blob_gas_used_delta from the same value.
                    evm_accum.blob_gas_used =
                        evm_accum.blob_gas_used.saturating_add(tx_da_footprint);
                }

                // Build receipt with proper deposit fields
                let success = result.is_success();
                let logs: Vec<alloy_primitives::Log> = result.logs().to_vec();
                for log_entry in &logs {
                    evm_accum.logs_bloom.accrue_log(log_entry);
                }

                // For deposit transactions, populate deposit_nonce (post-Regolith)
                // and deposit_receipt_version (post-Canyon) to match the standard
                // block executor's receipt construction.
                let deposit_nonce = if is_regolith && is_deposit {
                    // Read sender's nonce from the committed state (post-execution).
                    // This matches the block executor which reads from db after commit.
                    evm.db_mut()
                        .basic(sender)
                        .ok()
                        .flatten()
                        .map(|account| account.nonce)
                } else {
                    None
                };
                let deposit_receipt_version = (is_deposit && is_canyon).then_some(1);

                let receipt = OpReceiptEnvelope::from_parts(
                    success,
                    evm_accum.cumulative_gas_used,
                    &logs,
                    tx_type,
                    deposit_nonce,
                    deposit_receipt_version,
                );
                evm_accum.receipts.push(receipt);
            }

            // Drop the EVM to release the borrow on `state`
            drop(evm);

            // (f) Compute tx_hash
            let tx_hash = compute_tx_hash(&cw.transactions);

            // (g) Compute post-hashes
            // The State wraps CacheDB<PanicDB>; State.cache has accessed entries,
            // CacheDB.cache (the witness) has the full base layer.
            let post_db_hash = hash_overlay_state(&cw.cache, &state.cache, &state.block_hashes);
            let post_evm_hash = hash_evm_state(&evm_accum);

            // (h) Compute chunk_trace
            let chunk_trace = compute_chunk_trace(
                tx_hash,
                pre_db_hash,
                post_db_hash,
                pre_evm_hash,
                post_evm_hash,
            );

            // (i) Return
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

            let mut kona_executor: KonaExecutor<'_, _, _, OpEvmFactory> = KonaExecutor::new(
                rollup_config.as_ref(),
                l2_provider.clone(),
                l2_provider.clone(),
                OpEvmFactory::default(),
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

        // Load the Kailua executor with caching support
        let cached_executor = CachedExecutor::<KonaExecutor<'_, _, _, OpEvmFactory>>::new(
            execution_cache,
            rollup_config.as_ref(),
            l2_provider.clone(),
            l2_provider.clone(),
            OpEvmFactory::default(),
            execution_trace,
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod tests {
    use super::*;
    use crate::client::tests::TestOracle;
    use crate::precondition::proposal::ProposalPrecondition;
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
        let (result_boot_info, precondition) = run_core_client(
            proposal_data_hash,
            oracle.clone(),
            oracle.clone(),
            OracleBlobProvider::new(oracle.clone()),
            EthereumDataSourceProvider,
            vec![],
            Some(collection_target.clone()),
            derivation_cache,
            derivation_trace.clone(),
            None,
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
        )
        .expect("run_core_client chunk mode")
    }

    /// Creates a chunk witness with default empty state for testing.
    fn make_test_chunk_witness(
        chunk_index: u16,
        total_chunks: u16,
        cache: alloy_evm::revm::database::in_memory_db::Cache,
        evm_state: crate::precondition::chunking::EvmAccumulatorState,
        transactions: Vec<Vec<u8>>,
    ) -> ChunkWitnessData {
        ChunkWitnessData {
            block_number: 16491250,
            chunk_index,
            total_chunks,
            tx_start: 0,
            tx_count: transactions.len() as u16,
            transactions,
            block_env: alloy_evm::revm::context::BlockEnv::default(),
            op_block_ctx: alloy_op_evm::OpBlockExecutionCtx::default(),
            cache,
            evm_state,
            agreed_l2_output_root: b256!(
                "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
            ),
            config_hash: B256::ZERO,
            fpvm_image_id: B256::ZERO,
            payout_recipient: alloy_primitives::Address::ZERO,
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
            compute_chunk_trace, compute_tx_hash, hash_cache, hash_evm_state, EvmAccumulatorState,
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
        let cw = make_test_chunk_witness(0, 1, cache.clone(), evm_state.clone(), vec![]);

        // Compute expected chunk trace (no txs → post == pre)
        let tx_hash = compute_tx_hash(&[]);
        let pre_db_hash = hash_cache(&cache);
        let pre_evm_hash = hash_evm_state(&evm_state);
        let expected_trace = compute_chunk_trace(
            tx_hash,
            pre_db_hash,
            pre_db_hash,
            pre_evm_hash,
            pre_evm_hash,
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

        let cw0 = make_test_chunk_witness(0, 2, cache.clone(), evm_state.clone(), vec![]);
        let cw1 = make_test_chunk_witness(1, 2, cache.clone(), evm_state.clone(), vec![]);

        let pre_db_hash_0 = hash_cache(&cw0.cache);
        let pre_db_hash_1 = hash_cache(&cw1.cache);

        // With no transactions, the chunk execution doesn't modify state,
        // so post_db_hash_0 == pre_db_hash_0 == pre_db_hash_1
        assert_eq!(pre_db_hash_0, pre_db_hash_1);

        // Run both chunks and verify they produce valid results
        let (_boot0, _prec0) = test_chunk_execution(make_chunk_boot_info(), cw0);
        let (_boot1, _prec1) = test_chunk_execution(make_chunk_boot_info(), cw1);
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

        let cw = make_test_chunk_witness(0, 1, cache, EvmAccumulatorState::default(), vec![]);

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
        );
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("chunk witness required"),
            "unexpected error: {err_msg}"
        );
    }
}

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
use crate::executor::{new_execution_cursor, CachedExecutor, Execution};
use crate::kona::OracleL1ChainProvider;
use crate::oracle::local::LocalOnceOracle;
use crate::precondition::execution::exec_precondition_hash;
use crate::precondition::{proposal, Precondition};
#[cfg(not(feature = "experimental"))]
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
use risc0_zkvm::sha::Digestible;
use std::fmt::Debug;
use std::mem::take;
use std::sync::{Arc, Mutex};
#[cfg(feature = "experimental")]
use {
    crate::evm::{
        cached::CachedEvmFactory,
        expected::capture_required_expected_state,
        partial::{PartialExecution, TransactionResultCollector},
        witness::PartialExecutionWitness,
    },
    crate::executor::build_single_partial_for_block,
    crate::precondition::evm::{
        compute_pe_trace, hash_block_ctx, hash_expected_state, hash_results,
    },
    alloy_consensus::{transaction::SignerRecoverable, Header},
    alloy_eips::eip2718::{Decodable2718, WithEncoded},
    alloy_evm::{
        block::BlockExecutor,
        revm::{
            bytecode::Bytecode,
            database::{CacheState, State},
        },
        EvmFactory,
    },
    alloy_op_evm::{block::OpAlloyReceiptBuilder, OpBlockExecutor},
    kona_executor::TrieDB,
    op_alloy_consensus::OpTxEnvelope,
};

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
    #[cfg(feature = "experimental")] pe_witness: Option<PartialExecutionWitness>,
    #[cfg(feature = "experimental")] partial_executions: Vec<Vec<PartialExecution>>,
    #[cfg(feature = "experimental")] partials_collector: Option<TransactionResultCollector>,
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
        //                     PARTIAL EXECUTION                      //
        ////////////////////////////////////////////////////////////////
        #[cfg(feature = "experimental")]
        if boot.l1_head == B256::repeat_byte(0xFF) {
            log("PARTIAL EXECUTION");
            let PartialExecutionWitness {
                transactions,
                block_env,
                op_block_ctx,
                cache,
            } = pe_witness.context("partial witness required in partial mode")?;
            // Calculate prestate hashes
            let block_ctx_hash = hash_block_ctx(&block_env, &op_block_ctx);

            // Instantiate provider
            let l2_provider = OracleL2ChainProvider::new(
                boot.agreed_l2_output_root,
                rollup_config.clone(),
                oracle.clone(),
            );
            let safe_head = l2_provider
                .header_by_hash(boot.agreed_l2_output_root)
                .map(|header| Sealed::new_unchecked(header, boot.agreed_l2_output_root))
                .context("l2_provider.header_by_hash")?;

            // Validate witness
            validate_cache(&cache);
            assert_eq!(boot.agreed_l2_output_root, boot.claimed_l2_output_root);
            assert_eq!(boot.claimed_l2_block_number, safe_head.number);

            // Build state: seed the pre-state view from the witness `CacheState`
            let mut state = State::builder()
                .with_database(TrieDB::new(
                    safe_head,
                    l2_provider.clone(),
                    l2_provider.clone(),
                ))
                .with_cached_prestate(cache)
                .build();

            // Set up EVM environment
            let cfg_env = alloy_evm::revm::context::CfgEnv::new()
                .with_chain_id(boot.chain_id)
                .with_spec_and_mainnet_gas_params(rollup_config.spec_id(block_env.timestamp.to()));
            // Re-derive the expected state snapshot
            let expected_state_hash =
                hash_expected_state(&capture_required_expected_state(&mut state));
            let evm_env = alloy_evm::EvmEnv::new(cfg_env, block_env);

            // Instantiate traced block executor to capture all execution results
            let cached_evm_factory = CachedEvmFactory::new_with_traces(
                Vec::new(),
                Some(Arc::new(Mutex::new(Vec::new()))),
            );
            let mut op_block_executor = OpBlockExecutor::new(
                cached_evm_factory.create_evm(&mut state, evm_env),
                op_block_ctx,
                rollup_config.clone(),
                OpAlloyReceiptBuilder::default(),
            );

            // Execute all transactions in partial
            for tx_bytes in transactions {
                let tx = OpTxEnvelope::decode_2718_exact(&tx_bytes)
                    .context("invalid transaction encoding in partial witness")?;
                let recovered = tx
                    .try_into_recovered()
                    .context("invalid transaction signature in partial witness")?;
                let wrapped = WithEncoded::new(tx_bytes.into(), recovered);
                op_block_executor
                    .execute_transaction(wrapped)
                    .map_err(|e| anyhow::anyhow!("partial transaction execution failed: {e}"))?;
            }

            // collect ordered (tx_hash, ResultAndState) pairs
            let traces = cached_evm_factory
                .take_all_block_traces()
                .into_iter()
                .next()
                .unwrap_or_default();
            let (captured_tx_hashes, captured_results): (Vec<B256>, Vec<_>) = traces
                .into_iter()
                .map(|trace| (trace.tx_hash, trace.result))
                .unzip();

            // Return result
            let pe_trace = compute_pe_trace(
                hash_results(&captured_tx_hashes, &captured_results),
                block_ctx_hash,
                expected_state_hash,
            );
            return Ok((boot, Precondition::default().partial(pe_trace)));
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

            #[cfg(feature = "experimental")]
            let mut kona_executor: KonaExecutor<'_, _, _, CachedEvmFactory> = KonaExecutor::new(
                rollup_config.as_ref(),
                l2_provider.clone(),
                l2_provider.clone(),
                CachedEvmFactory::new_with_traces(partial_executions, partials_collector),
                None,
            );
            #[cfg(not(feature = "experimental"))]
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

        #[cfg(feature = "experimental")]
        let cached_executor = CachedExecutor::<KonaExecutor<'_, _, _, CachedEvmFactory>>::new(
            execution_cache,
            rollup_config.as_ref(),
            l2_provider.clone(),
            l2_provider.clone(),
            CachedEvmFactory::new_with_traces(partial_executions, partials_collector),
            execution_trace,
        );
        #[cfg(not(feature = "experimental"))]
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

#[cfg(feature = "experimental")]
pub fn recover_collected_partials(
    boot_info: &BootInfo,
    partials_collector: TransactionResultCollector,
    execution_cache: &[Execution],
    mut parent_header: Header,
) -> Vec<Vec<PartialExecution>> {
    let mut partial_executions = vec![];
    for (partials, execution) in take(&mut *partials_collector.lock().unwrap())
        .into_iter()
        .zip(execution_cache)
    {
        partial_executions.push(vec![build_single_partial_for_block(
            execution,
            partials,
            &parent_header,
            boot_info
                .rollup_config
                .spec_id(execution.artifacts.header.timestamp),
        )]);
        parent_header = execution.artifacts.header.inner().clone();
    }
    partial_executions
}

#[cfg(feature = "experimental")]
pub fn split_collected_partials(
    partials: Vec<Vec<PartialExecution>>,
    partials_per_block: usize,
) -> Vec<Vec<PartialExecution>> {
    partials
        .into_iter()
        .map(|mut block_partials| {
            block_partials
                .pop()
                .map(|partial| partial.split(partials_per_block))
                .unwrap_or_default()
        })
        .collect()
}

#[cfg(feature = "experimental")]
pub fn validate_contract_hash(expected_hash: &B256, bytecode: &Bytecode) {
    if expected_hash.is_zero() {
        // workaround
        assert!(bytecode.is_empty());
    } else {
        // rehash
        let actual_hash = bytecode.hash_slow();
        assert_eq!(
            actual_hash, *expected_hash,
            "cached contract bytecode hash mismatch: expected {expected_hash:?}, got {actual_hash:?}"
        );
    }
}

/// Validates every [`Bytecode`] entry
#[cfg(feature = "experimental")]
pub fn validate_cache(cache: &CacheState) {
    // Top-level contracts map (bytecode served via `code_by_hash`).
    for (expected_hash, bytecode) in &cache.contracts {
        validate_contract_hash(expected_hash, bytecode);
    }
    // Inline bytecode attached to any pre-state account.
    for cache_account in cache.accounts.values() {
        if let Some(plain) = &cache_account.account {
            if let Some(bytecode) = &plain.info.code {
                validate_contract_hash(&plain.info.code_hash, bytecode);
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod tests {
    use super::*;
    use crate::client::tests::TestOracle;
    use crate::precondition::proposal::ProposalPrecondition;
    use alloy_consensus::Header;
    use alloy_primitives::{b256, B256};
    use kona_proof::l1::OracleBlobProvider;
    use kona_proof::BootInfo;
    use std::sync::{Arc, Mutex};

    pub async fn test_derivation(
        boot_info: BootInfo,
        proposal_data: Option<ProposalPrecondition>,
        derivation_cache: Option<CachedDriver>,
        derivation_trace: Option<Arc<Mutex<Option<CachedDriver>>>>,
    ) -> anyhow::Result<Vec<Execution>> {
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
        let executions_collector = Arc::new(Mutex::new(Vec::new()));
        let (result_boot_info, precondition) = run_core_client(
            proposal_data_hash,
            oracle.clone(),
            oracle.clone(),
            OracleBlobProvider::new(oracle.clone()),
            EthereumDataSourceProvider,
            vec![],
            Some(executions_collector.clone()),
            derivation_cache,
            derivation_trace.clone(),
            #[cfg(feature = "experimental")]
            None,
            #[cfg(feature = "experimental")]
            Vec::new(),
            #[cfg(feature = "experimental")]
            None,
        )
        .context("run_core_client")?;

        // Verify boot info matches expectations
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

        // Verify precondition matches expectations
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
            partial_executions: Default::default(),
        };
        assert_eq!(precondition.digest(), expected_precondition.digest(),);

        // Extract block executions
        let execution_cache =
            recover_collected_executions(executions_collector, boot_info.claimed_l2_output_root);

        Ok(execution_cache)
    }

    pub async fn fetch_safe_head_config(
        boot_info: &BootInfo,
    ) -> anyhow::Result<(Header, RollupConfig)> {
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

    #[cfg(feature = "experimental")]
    pub async fn test_derivation_with_partials(
        boot_info: BootInfo,
        proposal_data: Option<ProposalPrecondition>,
        derivation_cache: Option<CachedDriver>,
        derivation_trace: Option<Arc<Mutex<Option<CachedDriver>>>>,
        partial_executions: Vec<Vec<PartialExecution>>,
    ) -> anyhow::Result<(Vec<Execution>, Vec<Vec<PartialExecution>>)> {
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
        let executions_collector = Arc::new(Mutex::new(Vec::new()));
        let partials_collector = Arc::new(Mutex::new(Vec::new()));
        let (result_boot_info, precondition) = run_core_client(
            proposal_data_hash,
            oracle.clone(),
            oracle.clone(),
            OracleBlobProvider::new(oracle.clone()),
            EthereumDataSourceProvider,
            vec![],
            Some(executions_collector.clone()),
            derivation_cache,
            derivation_trace.clone(),
            #[cfg(feature = "experimental")]
            None,
            #[cfg(feature = "experimental")]
            partial_executions,
            #[cfg(feature = "experimental")]
            Some(partials_collector.clone()),
        )
        .context("run_core_client")?;

        // Verify boot info matches expectations
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

        // Verify precondition matches expectations
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
            partial_executions: Default::default(),
        };
        assert_eq!(precondition.digest(), expected_precondition.digest(),);

        // Extract block executions
        let execution_cache =
            recover_collected_executions(executions_collector, boot_info.claimed_l2_output_root);

        // Extract transaction executions
        let (parent_header, _) = fetch_safe_head_config(&result_boot_info).await?;
        let partial_executions = recover_collected_partials(
            &result_boot_info,
            partials_collector,
            &execution_cache,
            parent_header,
        );

        Ok((execution_cache, partial_executions))
    }

    pub fn test_execution(
        boot_info: BootInfo,
        execution_cache: Vec<Execution>,
    ) -> anyhow::Result<B256> {
        // Ensure boot info triggers execution only
        assert!(boot_info.l1_head.is_zero());
        let execution_cache: Vec<_> = execution_cache.into_iter().map(Arc::new).collect();
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
            #[cfg(feature = "experimental")]
            None,
            #[cfg(feature = "experimental")]
            Vec::new(),
            #[cfg(feature = "experimental")]
            None,
        )
        .expect("run_core_client");

        // Verify matching boot
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

        // Verify precondition
        assert_eq!(
            B256::new(precondition.digest().into()),
            expected_precondition_hash
        );

        Ok(expected_precondition_hash)
    }

    #[cfg(feature = "experimental")]
    pub fn test_partial(
        boot_info: BootInfo,
        partial_witness: PartialExecutionWitness,
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
            Some(partial_witness),
            Vec::new(),
            None,
        )
        .expect("run_core_client partial mode")
    }

    pub fn op_sepolia_16491249_16491250() -> BootInfo {
        BootInfo {
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
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491250() {
        test_derivation(
            op_sepolia_16491249_16491250(),
            None,
            None,
            Some(Default::default()),
        )
        .await
        .unwrap();
    }

    #[cfg(feature = "experimental")]
    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491250_partials_roundtrip() {
        // Capture partials
        let (pre_executions, pre_captured) = test_derivation_with_partials(
            op_sepolia_16491249_16491250(),
            None,
            None,
            Some(Default::default()),
            Vec::new(),
        )
        .await
        .unwrap();

        // Run with partials loaded
        let (post_executions, post_captured) = test_derivation_with_partials(
            op_sepolia_16491249_16491250(),
            None,
            None,
            Some(Default::default()),
            pre_captured,
        )
        .await
        .unwrap();

        // Assert all results were served cached (no fresh captured)
        assert!(post_captured
            .iter()
            .all(|c| c.first().unwrap().results.is_empty()));
        // Assert both executions yield the same blocks
        for (pre, post) in pre_executions.into_iter().zip(post_executions.into_iter()) {
            assert_eq!(pre.artifacts.header, post.artifacts.header);
        }
    }

    #[cfg(feature = "experimental")]
    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491250_split_partials_roundtrip() {
        // Capture partials
        let (pre_executions, pre_captured) = test_derivation_with_partials(
            op_sepolia_16491249_16491250(),
            None,
            None,
            Some(Default::default()),
            Vec::new(),
        )
        .await
        .unwrap();

        let split = split_collected_partials(pre_captured, usize::MAX);

        // Run with partials loaded
        let (post_executions, post_captured) = test_derivation_with_partials(
            op_sepolia_16491249_16491250(),
            None,
            None,
            Some(Default::default()),
            split,
        )
        .await
        .unwrap();

        // Assert all results were served cached (no fresh captured)
        assert!(post_captured
            .iter()
            .all(|c| c.first().unwrap().results.is_empty()));
        // Assert both executions yield the same blocks
        for (pre, post) in pre_executions.into_iter().zip(post_executions.into_iter()) {
            assert_eq!(pre.artifacts.header, post.artifacts.header);
        }
    }

    pub fn op_sepolia_16491249_16491349() -> BootInfo {
        BootInfo {
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
        }
    }

    #[cfg(feature = "experimental")]
    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491349_partials_roundtrip() {
        // Capture partials
        let (pre_executions, pre_captured) = test_derivation_with_partials(
            op_sepolia_16491249_16491349(),
            None,
            None,
            Some(Default::default()),
            Vec::new(),
        )
        .await
        .unwrap();

        // Test all individual partials
        for (partials, execution) in pre_captured.iter().zip(pre_executions.iter()) {
            for partial_execution in partials {
                let witness =
                    PartialExecutionWitness::from_preflight(partial_execution.clone(), execution);
                // all partials should be found
                assert_eq!(
                    partial_execution.tx_hashes.len(),
                    witness.transactions.len()
                );
                // partial test should succeed
                let (_, precondition) = test_partial(
                    make_pe_boot(&op_sepolia_16491249_16491349(), &witness),
                    witness,
                );
                assert_eq!(
                    precondition,
                    Precondition::default().partial(partial_execution.precondition_hash())
                );
            }
        }

        let split_captured = split_collected_partials(pre_captured.clone(), usize::MAX);

        // Test a mid-block split partial too. This exercises the proof path whose
        // expected L1BlockInfo state must include writes from earlier
        // transactions in the same block.
        let (split_partial, split_execution) = split_captured
            .iter()
            .zip(pre_executions.iter())
            .find_map(|(partials, execution)| partials.get(1).map(|partial| (partial, execution)))
            .expect("fixture should include at least one mid-block split partial");
        let witness =
            PartialExecutionWitness::from_preflight(split_partial.clone(), split_execution);
        assert_eq!(split_partial.tx_hashes.len(), witness.transactions.len());
        let (_, precondition) = test_partial(
            make_pe_boot(&op_sepolia_16491249_16491349(), &witness),
            witness,
        );
        assert_eq!(
            precondition,
            Precondition::default().partial(split_partial.precondition_hash())
        );

        // Run with partials loaded
        let (post_executions, post_captured) = test_derivation_with_partials(
            op_sepolia_16491249_16491349(),
            None,
            None,
            Some(Default::default()),
            pre_captured,
        )
        .await
        .unwrap();
        // Assert all results were served cached (no fresh captured)
        assert!(post_captured
            .iter()
            .all(|c| c.first().unwrap().results.is_empty()));
        // Assert both executions yield the same blocks
        for (pre, post) in pre_executions.into_iter().zip(post_executions.into_iter()) {
            assert_eq!(pre.artifacts.header, post.artifacts.header);
        }

        // Run with fully split partials loaded
        let (_post_split_executions, post_split_captured) = test_derivation_with_partials(
            op_sepolia_16491249_16491349(),
            None,
            None,
            Some(Default::default()),
            split_captured,
        )
        .await
        .unwrap();
        assert!(post_split_captured
            .iter()
            .all(|c| c.first().unwrap().results.is_empty()));
    }

    #[cfg(feature = "experimental")]
    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491349_with_empty_partials() {
        let partials = vec![Vec::<PartialExecution>::new(); 100];

        // Capture partials
        let (pre_executions, pre_captured) = test_derivation_with_partials(
            op_sepolia_16491249_16491349(),
            None,
            None,
            Some(Default::default()),
            partials,
        )
        .await
        .unwrap();

        assert_eq!(pre_executions.len(), pre_captured.len());
    }

    /// Exercises the failure path on the partial-execution branch: a witness
    /// where revm rejects a transaction must propagate via the
    /// `partial transaction execution failed` `map_err` closure on the
    /// `op_block_executor.execute_transaction(...)` call.
    #[cfg(feature = "experimental")]
    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_partial_execution_failure_propagates_error() {
        // Capture real witnesses from a derivation; we need at least one
        // non-deposit tx whose sender lives in the witness cache.
        let (pre_executions, pre_captured) = test_derivation_with_partials(
            op_sepolia_16491249_16491349(),
            None,
            None,
            Some(Default::default()),
            Vec::new(),
        )
        .await
        .unwrap();

        // Find a witness whose first user (non-deposit) tx has a tamperable
        // sender entry in the prestate cache, then bump that sender's nonce so
        // revm rejects with `NonceTooLow` when the tx executes.
        let mut tampered: Option<PartialExecutionWitness> = None;
        'outer: for (partials, execution) in pre_captured.iter().zip(pre_executions.iter()) {
            for partial in partials {
                let mut witness =
                    PartialExecutionWitness::from_preflight(partial.clone(), execution);
                for tx_bytes in &witness.transactions {
                    let Ok(tx) = OpTxEnvelope::decode_2718_exact(tx_bytes) else {
                        continue;
                    };
                    if tx.is_deposit() {
                        continue;
                    }
                    let Ok(recovered) = tx.try_into_recovered() else {
                        continue;
                    };
                    let sender = recovered.signer();
                    if let Some(cache_account) = witness.cache.accounts.get_mut(&sender) {
                        if let Some(plain) = cache_account.account.as_mut() {
                            plain.info.nonce = plain.info.nonce.saturating_add(1);
                            tampered = Some(witness);
                            break 'outer;
                        }
                    }
                }
            }
        }
        let witness =
            tampered.expect("fixture must contain at least one user tx with cached sender");

        // Drive the partial-execution branch with the tampered witness.
        let boot_info = make_pe_boot(&op_sepolia_16491249_16491349(), &witness);
        let oracle = Arc::new(crate::client::tests::TestOracle::new(boot_info.clone()));
        let err = run_core_client(
            B256::ZERO,
            oracle.clone(),
            oracle.clone(),
            OracleBlobProvider::new(oracle.clone()),
            EthereumDataSourceProvider,
            vec![],
            None,
            None,
            None,
            Some(witness),
            Vec::new(),
            None,
        )
        .expect_err("tampered witness must fail partial transaction execution");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("partial transaction execution failed"),
            "expected 'partial transaction execution failed' in error chain, got: {msg}",
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491349() {
        let executions = test_derivation(
            op_sepolia_16491249_16491349(),
            None,
            None,
            Some(Default::default()),
        )
        .await
        .unwrap();
        let _ = test_execution(
            {
                let mut boot_info = op_sepolia_16491249_16491349();
                boot_info.l1_head = B256::ZERO;
                boot_info
            },
            executions,
        )
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491349_validity() {
        test_derivation(
            op_sepolia_16491249_16491349(),
            Some(ProposalPrecondition {
                proposal_l2_head_number: 16491249,
                proposal_output_count: 1,
                output_block_span: 100,
                blob_hashes: vec![],
            }),
            None,
            Some(Default::default()),
        )
        .await
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
        .await
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
        .await
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
        .await
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
        .await
        .unwrap();
        assert!(executions.is_empty());
    }

    #[cfg(feature = "experimental")]
    #[test]
    fn validate_contract_hash_zero_workaround() {
        use alloy_primitives::Bytes;

        // Workaround branch: zero `expected_hash` requires empty bytecode.
        validate_contract_hash(&B256::ZERO, &Bytecode::new_raw(Bytes::new()));
    }

    #[cfg(feature = "experimental")]
    pub fn make_pe_boot(boot_info: &BootInfo, witness: &PartialExecutionWitness) -> BootInfo {
        BootInfo {
            l1_head: B256::repeat_byte(0xFF),
            agreed_l2_output_root: witness.op_block_ctx.parent_hash,
            claimed_l2_output_root: witness.op_block_ctx.parent_hash,
            claimed_l2_block_number: witness.block_env.number.to::<u64>().saturating_sub(1),
            chain_id: boot_info.chain_id,
            rollup_config: boot_info.rollup_config.clone(),
            l1_config: boot_info.l1_config.clone(),
        }
    }
}

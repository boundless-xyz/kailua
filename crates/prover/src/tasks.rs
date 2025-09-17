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

use crate::args::ProveArgs;
use crate::driver::{driver_file_name, signal_derivation_trace, try_read_driver};
use crate::kv::RWLKeyValueStore;
use crate::proof::{proof_file_name, read_bincoded_file};
use crate::ProvingError;
use alloy::providers::RootProvider;
use alloy_primitives::B256;
use anyhow::{anyhow, Context};
use async_channel::{Receiver, Sender};
use human_bytes::human_bytes;
use kailua_kona::boot::StitchedBootInfo;
use kailua_kona::client::stitching::{split_executions, stitch_boot_info};
use kailua_kona::driver::CachedDriver;
use kailua_kona::executor::Execution;
use kailua_kona::oracle::vec::VecOracle;
use kailua_kona::precondition::execution::exec_precondition_hash;
use kailua_kona::precondition::Precondition;
use kailua_sync::provider::optimism::OpNodeProvider;
use kona_genesis::RollupConfig;
use kona_proof::BootInfo;
use risc0_zkvm::sha::Digestible;
use risc0_zkvm::Receipt;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::convert::identity;
use std::path::Path;
use std::sync::Arc;
use tracing::{error, info, warn};

#[derive(Clone, Debug)]
pub struct CachedTask {
    pub args: ProveArgs,
    pub rollup_config: RollupConfig,
    pub disk_kv_store: Option<RWLKeyValueStore>,
    pub precondition: Precondition,
    pub proposal_data_hash: B256,
    pub stitched_executions: Vec<Vec<Execution>>,
    pub derivation_cache: Option<CachedDriver>,
    pub derivation_trace: Option<Sender<CachedDriver>>,
    pub stitched_preconditions: Vec<Precondition>,
    pub stitched_boot_info: Vec<StitchedBootInfo>,
    pub stitched_proofs: Vec<Receipt>,
    pub prove_snark: bool,
    pub force_attempt: bool,
    pub seek_proof: bool,
}

impl PartialEq for CachedTask {
    fn eq(&self, other: &Self) -> bool {
        self.args.eq(&other.args)
    }
}

impl Eq for CachedTask {}

impl PartialOrd for CachedTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CachedTask {
    fn cmp(&self, other: &Self) -> Ordering {
        self.args.cmp(&other.args)
    }
}

pub type OneshotResultResponse = (Receipt, Precondition);

#[derive(Debug)]
pub struct OneshotResult {
    pub cached_task: CachedTask,
    pub result: Result<OneshotResultResponse, ProvingError>,
}

impl PartialEq for OneshotResult {
    fn eq(&self, other: &Self) -> bool {
        self.cached_task.eq(&other.cached_task)
    }
}

impl Eq for OneshotResult {}

impl PartialOrd for OneshotResult {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OneshotResult {
    fn cmp(&self, other: &Self) -> Ordering {
        self.cached_task.cmp(&other.cached_task)
    }
}

#[derive(Debug)]
pub struct Oneshot {
    pub cached_task: CachedTask,
    pub result_sender: Sender<OneshotResult>,
}

/// Indefinitely processes incoming [Oneshot] messages via the provided channel
pub async fn handle_oneshot_tasks(task_receiver: Receiver<Oneshot>) -> anyhow::Result<()> {
    loop {
        let Oneshot {
            cached_task,
            result_sender,
        } = task_receiver
            .recv()
            .await
            .context("task receiver channel closed")?;

        if let Err(res) = result_sender
            .send(OneshotResult {
                cached_task: cached_task.clone(),
                result: compute_cached_proof(
                    cached_task.args,
                    cached_task.rollup_config,
                    cached_task.disk_kv_store,
                    cached_task.precondition,
                    cached_task.proposal_data_hash,
                    cached_task.stitched_executions,
                    cached_task.derivation_cache,
                    cached_task.derivation_trace,
                    cached_task.stitched_preconditions,
                    cached_task.stitched_boot_info,
                    cached_task.stitched_proofs,
                    cached_task.prove_snark,
                    cached_task.force_attempt,
                    cached_task.seek_proof,
                )
                .await,
            })
            .await
        {
            error!("failed to send task result: {res:?}");
        }
    }
}

/// Send a [Oneshot] task to the prover pool and return once the result arrives
#[allow(clippy::too_many_arguments)]
pub async fn compute_oneshot_task(
    args: ProveArgs,
    rollup_config: RollupConfig,
    disk_kv_store: Option<RWLKeyValueStore>,
    precondition: Precondition,
    proposal_data_hash: B256,
    stitched_executions: Vec<Vec<Execution>>,
    derivation_cache: Option<CachedDriver>,
    derivation_trace: Option<Sender<CachedDriver>>,
    stitched_preconditions: Vec<Precondition>,
    stitched_boot_info: Vec<StitchedBootInfo>,
    stitched_proofs: Vec<Receipt>,
    prove_snark: bool,
    force_attempt: bool,
    seek_proof: bool,
    task_sender: Sender<Oneshot>,
) -> Result<OneshotResultResponse, ProvingError> {
    // create proving task
    let cached_task = CachedTask {
        args,
        rollup_config,
        disk_kv_store,
        precondition,
        proposal_data_hash,
        stitched_executions,
        derivation_cache,
        derivation_trace,
        stitched_preconditions,
        stitched_boot_info,
        stitched_proofs,
        prove_snark,
        force_attempt,
        seek_proof,
    };
    // create oneshot channel
    let oneshot_channel = async_channel::bounded(1);
    // dispatch task to pool
    task_sender
        .send(Oneshot {
            cached_task,
            result_sender: oneshot_channel.0,
        })
        .await
        .expect("Oneshot channel closed");
    // wait for result
    oneshot_channel
        .1
        .recv()
        .await
        .expect("oneshot_channel should never panic")
        .result
}

/// Computes a receipt if it is not cached
#[allow(clippy::too_many_arguments)]
pub async fn compute_fpvm_proof(
    args: ProveArgs,
    rollup_config: RollupConfig,
    disk_kv_store: Option<RWLKeyValueStore>,
    mut precondition: Precondition,
    proposal_data_hash: B256,
    derivation_cache: Option<Receiver<CachedDriver>>,
    derivation_trace: Option<Sender<CachedDriver>>,
    stitched_preconditions: Vec<Precondition>,
    stitched_boot_info: Vec<StitchedBootInfo>,
    stitched_proofs: Vec<Receipt>,
    prove_snark: bool,
    task_sender: Sender<Oneshot>,
) -> Result<Option<OneshotResultResponse>, ProvingError> {
    // report transaction count
    if !stitched_boot_info.is_empty() {
        info!("Stitching {} sub-proofs", stitched_boot_info.len());
    }
    if stitched_boot_info.len() != stitched_preconditions.len() {
        warn!(
            "Attempting to stitch {} sub-proofs with {} preconditions",
            stitched_boot_info.len(),
            stitched_preconditions.len()
        );
    }

    //  1. try entire proof
    //      on failure, take execution trace
    //      on success, signal driver trace
    //  2. try derivation-only proof
    //      on failure, report error
    //      on success, signal driver trace
    //  3. compute series of execution-only proofs
    //  4. compute derivation-proof with stitched executions

    // Wait for the cached driver to be reported before derivation unless it is skipped
    let derivation_cache = if !args.proving.skip_derivation_proof {
        match derivation_cache {
            Some(receiver) => {
                match receiver.recv().await {
                    Ok(cached_driver) => {
                        let derivation_cache_precondition =
                            B256::new(cached_driver.digest().into());
                        info!("Received CachedDriver {derivation_cache_precondition}");
                        // add cached driver precondition
                        precondition.derivation_cache = derivation_cache_precondition;
                        Some(cached_driver)
                    }
                    Err(err) => {
                        error!("Failed to receive CachedDriver: {err:?}. Proceeding with fresh derivation.");
                        None
                    }
                }
            }
            None => None,
        }
    } else {
        None
    };

    let stitching_only = args.kona.agreed_l2_output_root == args.kona.claimed_l2_output_root;
    // generate master proof
    info!("Attempting complete proof.");
    let complete_proof_result = compute_oneshot_task(
        args.clone(),
        rollup_config.clone(),
        disk_kv_store.clone(),
        precondition,
        proposal_data_hash,
        vec![],
        derivation_cache.clone(),
        derivation_trace, // note: the task sends its driver trace if it succeeds
        stitched_preconditions.clone(),
        stitched_boot_info.clone(),
        stitched_proofs.clone(),
        // pass through snark requirement
        prove_snark,
        // force attempting to compute the proof if it only combines boot infos
        stitching_only,
        // skip seeking a complete proof if not proving derivation
        !args.proving.skip_derivation_proof,
        task_sender.clone(),
    )
    .await;

    // on WitnessSizeError or NotSeekingProof, extract execution and derivation traces
    let (executed_blocks, derivation_trace) = match complete_proof_result {
        Err(ProvingError::BlockCountError(_, _, executed_blocks, _, derivation_trace)) => {
            (executed_blocks, derivation_trace)
        }
        Err(ProvingError::WitnessSizeError(_, _, executed_blocks, _, derivation_trace)) => {
            (executed_blocks, derivation_trace)
        }
        Err(ProvingError::NotSeekingProof(_, executed_blocks, _, derivation_trace, _)) => {
            (executed_blocks, derivation_trace)
        }
        other_result => return Ok(Some(other_result?)),
    };
    // Check if we can do execution-only proofs
    if args.proving.max_block_executions == 0 {
        return Err(ProvingError::OtherError(anyhow!(
            "Execution only proofs are disabled because max_block_executions=0."
        )));
    }
    // flatten executed l2 blocks
    let (_, execution_cache) = split_executions(executed_blocks.clone());

    // perform a derivation-only run to check its provability unless not proving derivation
    if !args.proving.skip_derivation_proof {
        info!(
            "Performing derivation-only run for {} executions.",
            execution_cache.len()
        );
        let derivation_only_result = compute_oneshot_task(
            args.clone(),
            rollup_config.clone(),
            disk_kv_store.clone(),
            precondition,
            proposal_data_hash,
            executed_blocks.clone(),
            derivation_cache.clone(),
            derivation_trace, // note: the task sends its driver trace if it succeeds
            stitched_preconditions.clone(),
            stitched_boot_info.clone(),
            stitched_proofs.clone(),
            false,
            false,
            false,
            task_sender.clone(),
        )
        .await;
        // propagate unexpected error up on failure to trigger higher-level division
        let Err(ProvingError::NotSeekingProof(witness_size, .., derivation_trace_hash)) =
            derivation_only_result
        else {
            warn!(
                "Unexpected derivation-only result (is_ok={}).",
                derivation_only_result.is_ok()
            );
            return Ok(Some(derivation_only_result?));
        };
        // update precondition
        if precondition.derivation_trace.is_zero() {
            precondition.derivation_trace = derivation_trace_hash;
        }

        // warn if pure derivation witness exceeds limit
        if witness_size > args.proving.max_witness_size {
            // todo: investigate if this is reachable.
            warn!(
                "Derivation-only witness size {} exceeds limit {}.",
                human_bytes(witness_size as f64),
                human_bytes(args.proving.max_witness_size as f64)
            );
        } else {
            info!(
                "Derivation-only witness size {}.",
                human_bytes(witness_size as f64)
            );
        }
    }

    // create results channel
    let result_channel = async_channel::unbounded();
    let mut result_pq = BinaryHeap::new();
    // divide and conquer executions
    let mut num_proofs = 0;
    let mut next_claim_index = args.proving.max_block_executions.min(execution_cache.len()) - 1;
    let mut agreed_l2_output_root = args.kona.agreed_l2_output_root;
    let mut agreed_l2_head_hash = args.kona.agreed_l2_head_hash;
    let last_claim_index = execution_cache.len() - 1;
    while agreed_l2_output_root != args.kona.claimed_l2_output_root {
        // Create sub-proof job
        let mut job_args = args.clone();
        job_args.kona.l1_head = B256::ZERO;
        job_args.kona.agreed_l2_output_root = agreed_l2_output_root;
        job_args.kona.agreed_l2_head_hash = agreed_l2_head_hash;
        job_args.kona.claimed_l2_output_root = execution_cache[next_claim_index].claimed_output;
        job_args.kona.claimed_l2_block_number =
            execution_cache[next_claim_index].artifacts.header.number;
        // advance pointers
        agreed_l2_output_root = job_args.kona.claimed_l2_output_root;
        agreed_l2_head_hash = execution_cache[next_claim_index].artifacts.header.hash();
        // queue up job
        num_proofs += 1;
        task_sender
            .send(Oneshot {
                cached_task: create_cached_execution_task(
                    job_args,
                    rollup_config.clone(),
                    disk_kv_store.clone(),
                    &execution_cache,
                ),
                result_sender: result_channel.0.clone(),
            })
            .await
            .expect("task_channel should not be closed");
        // next claim
        if next_claim_index == last_claim_index {
            break;
        }
        next_claim_index = next_claim_index
            .saturating_add(args.proving.max_block_executions)
            .min(last_claim_index);
    }
    // process execution-only proving results
    while result_pq.len() < num_proofs {
        // Wait for more proving results
        let oneshot_result = result_channel
            .1
            .recv()
            .await
            .expect("result_channel should not be closed");
        let Err(err) = oneshot_result.result else {
            result_pq.push(oneshot_result);
            continue;
        };
        // Require additional proof
        num_proofs += 1;
        let executed_blocks = oneshot_result.cached_task.stitched_executions[0].clone();
        let agreed_block = executed_blocks[0].artifacts.header.number - 1;
        let num_blocks =
            oneshot_result.cached_task.args.kona.claimed_l2_block_number - agreed_block;
        let forced_attempt = num_blocks == 1;
        // divide or bail out on error
        match err {
            ProvingError::WitnessSizeError(f, t, e, ..) => {
                if forced_attempt {
                    error!(
                        "Execution-only proof witness size {} above safety threshold {}.",
                        human_bytes(f as f64),
                        human_bytes(t as f64)
                    );
                    return Err(ProvingError::WitnessSizeError(
                        f,
                        t,
                        e,
                        Box::new(None),
                        None,
                    ));
                }
                warn!(
                    "Execution-only proof witness size {} above safety threshold {}. Splitting workload.",
                    human_bytes(f as f64),
                    human_bytes(t as f64)
                )
            }
            ProvingError::ExecutionError(e) => {
                if forced_attempt {
                    return Err(ProvingError::ExecutionError(e));
                }
                warn!("Splitting execution-only proof after ZKVM execution error: {e:?}")
            }
            ProvingError::OtherError(e) => {
                return Err(ProvingError::OtherError(e));
            }
            ProvingError::NotAwaitingProof => {
                // reduce required proofs by two to cancel out prior addition and one more proof
                num_proofs -= 2;
                continue;
            }
            ProvingError::BlockCountError(..) => {
                unreachable!("Unexpected BlockCountError {err:?}")
            }
            ProvingError::NotSeekingProof(..) => {
                unreachable!("Unexpected NotSeekingProof {err:?}")
            }
            ProvingError::DerivationProofError(_) => {
                unreachable!("Unexpected DerivationProofError {err:?}")
            }
        }
        // Split workload at midpoint (num_blocks > 1)
        let mid_point = agreed_block + num_blocks / 2;
        let mid_exec = executed_blocks
            .iter()
            .find(|e| e.artifacts.header.number == mid_point)
            .expect("Failed to find the midpoint of execution.");
        let mid_output = mid_exec.claimed_output;

        // Lower half workload ends at midpoint (inclusive)
        let mut lower_job_args = oneshot_result.cached_task.args.clone();
        lower_job_args.kona.claimed_l2_output_root = mid_output;
        lower_job_args.kona.claimed_l2_block_number = mid_point;
        task_sender
            .send(Oneshot {
                cached_task: create_cached_execution_task(
                    lower_job_args,
                    rollup_config.clone(),
                    disk_kv_store.clone(),
                    &execution_cache,
                ),
                result_sender: result_channel.0.clone(),
            })
            .await
            .expect("task_channel should not be closed");

        // upper half workload starts after midpoint
        let mut upper_job_args = oneshot_result.cached_task.args;
        upper_job_args.kona.agreed_l2_output_root = mid_output;
        upper_job_args.kona.agreed_l2_head_hash = mid_exec.artifacts.header.hash();
        task_sender
            .send(Oneshot {
                cached_task: create_cached_execution_task(
                    upper_job_args,
                    rollup_config.clone(),
                    disk_kv_store.clone(),
                    &execution_cache,
                ),
                result_sender: result_channel.0.clone(),
            })
            .await
            .expect("task_channel should not be closed");
    }
    // Read result_pq for stitched executions and proofs
    let (proofs, stitched_executions): (Vec<_>, Vec<_>) = result_pq
        .into_sorted_vec()
        .into_iter()
        .map(|mut r| {
            (
                r.result.expect("pushed failing result to queue").0,
                r.cached_task.stitched_executions.pop().unwrap(),
            )
        })
        .unzip();

    // Return proof count without stitching if derivation is not required
    if args.proving.skip_await_proof {
        warn!("Skipping stitching unawaited execution proofs with derivation.");
        return Err(ProvingError::NotAwaitingProof);
    } else if args.proving.skip_derivation_proof {
        let num_proofs = proofs.len();
        warn!("Skipping stitching {num_proofs} execution proofs with derivation.");
        return Err(ProvingError::DerivationProofError(num_proofs));
    }

    // Combine execution proofs with derivation proof
    let total_blocks = stitched_executions.iter().map(|e| e.len()).sum::<usize>();
    info!(
        "Stitching {}/{} execution proofs for {total_blocks} blocks with derivation proof.",
        proofs.len(),
        stitched_executions.len()
    );

    Ok(Some(
        compute_oneshot_task(
            args,
            rollup_config,
            disk_kv_store,
            precondition,
            proposal_data_hash,
            stitched_executions,
            derivation_cache,
            None,
            stitched_preconditions,
            stitched_boot_info,
            [stitched_proofs, proofs].concat(),
            prove_snark,
            true,
            true,
            task_sender.clone(),
        )
        .await?,
    ))
}

pub fn create_cached_execution_task(
    args: ProveArgs,
    rollup_config: RollupConfig,
    disk_kv_store: Option<RWLKeyValueStore>,
    execution_cache: &[Arc<Execution>],
) -> CachedTask {
    let starting_block = execution_cache
        .iter()
        .find(|e| e.agreed_output == args.kona.agreed_l2_output_root)
        .expect("Failed to find the first execution.")
        .artifacts
        .header
        .number
        - 1;
    let num_blocks = args.kona.claimed_l2_block_number - starting_block;
    info!(
        "Processing execution-only job with {} blocks from block {}",
        num_blocks, starting_block
    );
    // Extract executed slice
    let executed_blocks = execution_cache
        .iter()
        .filter(|e| {
            let executed_block_number = e.artifacts.header.number;

            starting_block < executed_block_number
                && executed_block_number <= args.kona.claimed_l2_block_number
        })
        .cloned()
        .collect::<Vec<_>>();
    let precondition =
        Precondition::default().execution(exec_precondition_hash(executed_blocks.as_slice()));

    // Force the proving attempt regardless of witness size if we prove just one block
    let force_attempt = num_blocks == 1;
    let executed_blocks = executed_blocks
        .iter()
        .map(|a| a.as_ref().clone())
        .collect::<Vec<_>>();

    CachedTask {
        args,
        rollup_config,
        disk_kv_store,
        precondition,
        proposal_data_hash: B256::ZERO,
        stitched_executions: vec![executed_blocks],
        derivation_cache: None,
        derivation_trace: None,
        stitched_preconditions: vec![],
        stitched_boot_info: vec![],
        stitched_proofs: vec![],
        prove_snark: false,
        force_attempt,
        seek_proof: true,
    }
}

/// Launches the native Kailua-Kona client-server pair to compute a [OneshotResultResponse]
#[allow(clippy::too_many_arguments)]
pub async fn compute_cached_proof(
    mut args: ProveArgs,
    rollup_config: RollupConfig,
    disk_kv_store: Option<RWLKeyValueStore>,
    mut precondition: Precondition,
    proposal_data_hash: B256,
    stitched_executions: Vec<Vec<Execution>>,
    derivation_cache: Option<CachedDriver>,
    mut derivation_trace: Option<Sender<CachedDriver>>,
    stitched_preconditions: Vec<Precondition>,
    stitched_boot_info: Vec<StitchedBootInfo>,
    stitched_proofs: Vec<Receipt>,
    prove_snark: bool,
    force_attempt: bool,
    seek_proof: bool,
) -> Result<OneshotResultResponse, ProvingError> {
    // extract single chain kona config
    let boot = BootInfo {
        l1_head: args.kona.l1_head,
        agreed_l2_output_root: args.kona.agreed_l2_output_root,
        claimed_l2_output_root: args.kona.claimed_l2_output_root,
        claimed_l2_block_number: args.kona.claimed_l2_block_number,
        chain_id: rollup_config.l2_chain_id,
        rollup_config,
    };
    // Choose image id
    let image_id = args.proving.image_id();

    // Check derivation driver cache if needed
    let driver_file = driver_file_name(image_id, &boot, &precondition);
    let trace_derivation = derivation_trace.is_some() || !precondition.derivation_trace.is_zero();
    if trace_derivation {
        if let Some(derivation_trace_hash) = signal_derivation_trace(
            derivation_trace.clone(),
            try_read_driver(&driver_file).await,
        )
        .await
        {
            // no need to double-send
            let _ = derivation_trace.take();
            // update precondition hash
            if precondition.derivation_trace.is_zero() {
                precondition.derivation_trace = derivation_trace_hash;
            } else if precondition.derivation_trace != derivation_trace_hash {
                warn!("Precondition derivation trace hash mismatch. Input: {}, Cached: {derivation_trace_hash}", precondition.derivation_trace);
            }
        }
    }

    // Construct expected journal
    // bug: this may fail when mixing proving with stitching boot infos
    let (boot, proof_journal, _) = stitch_boot_info::<VecOracle>(
        None, // assume l1 head chain continuity on host side
        boot,
        bytemuck::cast::<[u32; 8], [u8; 32]>(image_id).into(),
        args.proving.payout_recipient_address.unwrap_or_default(),
        precondition,
        stitched_preconditions.clone(),
        stitched_boot_info.clone(),
    )
    .await
    .context("Failed to stitch boot info")
    .map_err(ProvingError::OtherError)?;
    let skip_await_proof = args.proving.skip_await_proof;
    // Skip computation if previously saved to disk
    let mut proof_file = proof_file_name(image_id, &proof_journal);
    if Path::new(&proof_file).try_exists().is_ok_and(identity) && seek_proof {
        info!("Proving skipped. Proof file {proof_file} already exists.");
        // abort remainder of flow if no proof is to be awaited
        if skip_await_proof {
            return Err(ProvingError::NotAwaitingProof);
        }
    } else {
        if seek_proof {
            info!("Computing uncached proof {proof_file}.");
        } else {
            info!("Running native client.");
        }

        // preflight
        if args.kona.enable_experimental_witness_endpoint
            && !args.kona.is_offline()
            && args.op_node_address.is_some()
        {
            let l2_provider = args
                .kona
                .l2_node_address
                .as_ref()
                .map(|addr| {
                    RootProvider::new_http(
                        addr.as_str()
                            .try_into()
                            .expect("Failed to parse l2_node_address"),
                    )
                })
                .unwrap();
            let op_node_provider = args
                .op_node_address
                .as_ref()
                .map(|addr| {
                    OpNodeProvider(RootProvider::new_http(
                        addr.as_str()
                            .try_into()
                            .expect("Failed to parse op_node_address"),
                    ))
                })
                .unwrap();
            if crate::client::payload::run_payload_client(
                boot.clone(),
                l2_provider,
                op_node_provider,
                disk_kv_store.clone(),
            )
            .await
            .map_err(|e| ProvingError::OtherError(anyhow!(e)))?
            {
                // If we have used debug_executionWitness sucessfully then don't use Kona's
                // debug_executePayload logic as it doesn't have caching
                args.kona.enable_experimental_witness_endpoint = false;
            }
        }

        // generate a proof using the kailua client and kona server
        crate::client::native::run_native_client(
            args.clone(),
            disk_kv_store,
            proposal_data_hash,
            stitched_executions,
            derivation_cache,
            trace_derivation,
            derivation_trace,
            stitched_preconditions.clone(),
            stitched_boot_info.clone(),
            stitched_proofs,
            prove_snark,
            force_attempt,
            seek_proof,
        )
        .await?;
    }

    // Load cached driver if tracing derivation is required
    let derivation_trace = if trace_derivation {
        try_read_driver(&driver_file).await
    } else {
        None
    };
    // Correct precondition and target proof file if needed
    if precondition.derivation_trace.is_zero() && trace_derivation {
        if let Some(derivation_trace) = derivation_trace.as_ref() {
            // Update derivation trace precondition
            precondition.derivation_trace = B256::new(derivation_trace.digest().into());
            // Recalculate receipt file name with new precondition
            proof_file = proof_file_name(
                image_id,
                &stitch_boot_info::<VecOracle>(
                    None, // assume l1 head chain continuity on host side
                    boot,
                    bytemuck::cast::<[u32; 8], [u8; 32]>(image_id).into(),
                    args.proving.payout_recipient_address.unwrap_or_default(),
                    precondition,
                    stitched_preconditions,
                    stitched_boot_info,
                )
                .await
                .context("Failed to stitch boot info.")
                .map_err(ProvingError::OtherError)?
                .1,
            );
        }
    }
    // Load receipt
    let receipt = read_bincoded_file(&proof_file)
        .await
        .context(format!("Failed to read proof file {proof_file} contents."))
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    // Return combined response
    Ok((receipt, precondition))
}

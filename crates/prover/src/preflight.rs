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
use crate::kv::RWLKeyValueStore;
use crate::ProvingError;
use alloy::consensus::Transaction;
use alloy::eips::eip4844::IndexedBlobHash;
use alloy::eips::BlockNumberOrTag;
use alloy::providers::{Provider, RootProvider};
use alloy_primitives::B256;
use anyhow::{anyhow, bail, Context};
use kailua_kona::blobs::BlobFetchRequest;
use kailua_kona::journal::ProofJournal;
use kailua_kona::precondition::proposal::ProposalPrecondition;
use kailua_kona::precondition::Precondition;
use kailua_sync::provider::optimism::OpNodeProvider;
use kailua_sync::{await_tel, retry_res_ctx_timeout};
use kona_genesis::{L1ChainConfig, RollupConfig};
use kona_preimage::{PreimageKey, PreimageKeyType};
use kona_protocol::BlockInfo;
use opentelemetry::global::tracer;
use opentelemetry::trace::FutureExt;
use opentelemetry::trace::{TraceContextExt, Tracer};
use std::env::set_var;
use std::iter::zip;
use tracing::{error, info, warn};
use futures::future::join_all;
use futures::stream::{FuturesUnordered, StreamExt};

pub async fn get_blob_fetch_request(
    l1_provider: &RootProvider,
    l1_timeout: u64,
    block_hash: B256,
    blob_hash: B256,
) -> anyhow::Result<BlobFetchRequest> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("get_blob_fetch_request"));

    let block = await_tel!(
        context,
        tracer,
        "get_block_by_hash",
        retry_res_ctx_timeout!(
            l1_timeout,
            l1_provider
                .get_block_by_hash(block_hash)
                .full()
                .await
                .context("get_block_by_hash")?
                .ok_or_else(|| anyhow!("Failed to fetch starting block"))
        )
    );
    let mut blob_index = 0;
    let mut blob_found = false;
    for blob in block.transactions.into_transactions().flat_map(|tx| {
        tx.blob_versioned_hashes()
            .map(|h| h.to_vec())
            .unwrap_or_default()
    }) {
        if blob == blob_hash {
            blob_found = true;
            break;
        }
        blob_index += 1;
    }

    if !blob_found {
        bail!("Could not find blob with hash {blob_hash} in block {block_hash}");
    }

    Ok(BlobFetchRequest {
        block_ref: BlockInfo {
            hash: block.header.hash,
            number: block.header.number,
            parent_hash: block.header.parent_hash,
            timestamp: block.header.timestamp,
        },
        blob_hash: IndexedBlobHash {
            index: blob_index,
            hash: blob_hash,
        },
    })
}

pub async fn fetch_precondition_data(
    cfg: &ProveArgs,
) -> anyhow::Result<Option<ProposalPrecondition>> {
    // Determine precondition hash
    let hash_arguments = [
        cfg.precondition_params.is_empty(),
        cfg.precondition_block_hashes.is_empty(),
        cfg.precondition_blob_hashes.is_empty(),
    ];

    // fetch necessary data to validate blob equivalence precondition
    if hash_arguments.iter().all(|arg| !arg) {
        let providers =
            retry_res_ctx_timeout!(cfg.timeouts.max(), cfg.create_providers().await).await;
        if cfg.precondition_block_hashes.len() != cfg.precondition_blob_hashes.len() {
            bail!(
                "Blob reference mismatch. Found {} block hashes and {} blob hashes",
                cfg.precondition_block_hashes.len(),
                cfg.precondition_blob_hashes.len()
            );
        }

        let precondition_validation_data = if cfg.precondition_params.len() == 3 {
            let mut fetch_requests = Vec::with_capacity(cfg.precondition_block_hashes.len());
            for (block_hash, blob_hash) in zip(
                cfg.precondition_block_hashes.iter(),
                cfg.precondition_blob_hashes.iter(),
            ) {
                info!("Fetching blob hash {blob_hash} from block {block_hash}");
                fetch_requests.push(
                    get_blob_fetch_request(
                        &providers.l1,
                        cfg.timeouts.eth_rpc_timeout,
                        *block_hash,
                        *blob_hash,
                    )
                    .await?,
                );
            }
            ProposalPrecondition {
                proposal_l2_head_number: cfg.precondition_params[0],
                proposal_output_count: cfg.precondition_params[1],
                output_block_span: cfg.precondition_params[2],
                blob_hashes: fetch_requests,
            }
        } else {
            bail!("Too many precondition_params values provided");
        };

        let kv_store = cfg.kona.create_key_value_store()?;
        let mut store = kv_store.write().await;
        let hash = precondition_validation_data.hash();
        store.set(
            PreimageKey::new(*hash, PreimageKeyType::Sha256).into(),
            precondition_validation_data.to_vec(),
        )?;
        set_var("PRECONDITION_VALIDATION_DATA_HASH", hash.to_string());
        info!("Precondition data hash: {hash}");
        Ok(Some(precondition_validation_data))
    } else if hash_arguments.iter().any(|arg| !arg) {
        bail!("Insufficient number of arguments provided for precondition hash.")
    } else {
        warn!("Proving without a precondition hash.");
        Ok(None)
    }
}

/// Represents pre-fetched block metadata for a preflight job
struct PreflightJobMetadata {
    /// The target L2 block number for this job
    target_block: u64,
    /// The agreed L2 head hash (starting point)
    agreed_l2_head_hash: B256,
    /// The agreed L2 output root
    agreed_l2_output_root: B256,
    /// The claimed L2 output root (ending point)
    claimed_l2_output_root: B256,
}

#[allow(clippy::too_many_arguments)]
pub async fn concurrent_execution_preflight(
    args: &ProveArgs,
    rollup_config: RollupConfig,
    l1_config: L1ChainConfig,
    op_node_provider: &OpNodeProvider,
    disk_kv_store: Option<RWLKeyValueStore>,
) -> anyhow::Result<bool> {
    let tracer = tracer("kailua");
    let context =
        opentelemetry::Context::current_with_span(tracer.start("concurrent_execution_preflight"));

    let l2_provider = retry_res_ctx_timeout!(args.timeouts.max(), args.create_providers().await)
        .await
        .l2;
    let starting_block = await_tel!(
        context,
        tracer,
        "l2_provider get_block_by_hash agreed_l2_head_hash",
        retry_res_ctx_timeout!(
            args.timeouts.op_geth_timeout,
            l2_provider
                .get_block_by_hash(args.kona.agreed_l2_head_hash)
                .await
                .context("l2_provider get_block_by_hash agreed_l2_head_hash")?
                .ok_or_else(|| anyhow!("Failed to fetch agreed l2 block"))
        )
    )
    .header
    .number;

    let num_blocks = args.kona.claimed_l2_block_number - starting_block;
    if num_blocks == 0 {
        return Ok(true);
    }

    // Calculate block boundaries for each thread
    let blocks_per_thread = num_blocks / args.proving.num_concurrent_preflights;
    let extra_blocks = num_blocks % args.proving.num_concurrent_preflights;

    // Compute all boundary block numbers upfront
    let mut boundary_blocks = Vec::with_capacity(args.proving.num_concurrent_preflights as usize + 1);
    boundary_blocks.push(starting_block);
    let mut current_block = starting_block;
    for i in 0..args.proving.num_concurrent_preflights {
        let blocks_for_this_thread = blocks_per_thread + if i < extra_blocks { 1 } else { 0 };
        current_block += blocks_for_this_thread;
        boundary_blocks.push(current_block);
    }

    info!(
        "Prefetching metadata for {} boundary blocks in parallel",
        boundary_blocks.len()
    );

    // Prefetch all block hashes in parallel (we need hash for each boundary block)
    let block_hash_futures: Vec<_> = boundary_blocks
        .iter()
        .map(|&block_num| {
            let l2_provider = l2_provider.clone();
            let timeout = args.timeouts.op_geth_timeout;
            async move {
                retry_res_ctx_timeout!(
                    timeout,
                    l2_provider
                        .get_block_by_number(BlockNumberOrTag::Number(block_num))
                        .await
                        .context("get_block_by_number for prefetch")?
                        .ok_or_else(|| anyhow!("Failed to fetch block {block_num}"))
                )
                .await
            }
        })
        .collect();

    // Prefetch all output roots in parallel (we need output root for each boundary block)
    let output_root_futures: Vec<_> = boundary_blocks
        .iter()
        .map(|&block_num| {
            let op_node = op_node_provider.clone();
            let timeout = args.timeouts.op_node_timeout;
            async move {
                retry_res_ctx_timeout!(timeout, op_node.output_at_block(block_num).await).await
            }
        })
        .collect();

    // Execute all prefetch requests in parallel
    info!("Executing parallel prefetch for {} blocks", boundary_blocks.len());
    let (blocks, outputs) = tokio::join!(
        join_all(block_hash_futures),
        join_all(output_root_futures)
    );

    info!(
        "Prefetched {} blocks and {} output roots",
        blocks.len(),
        outputs.len()
    );

    // Build job metadata from prefetched data
    let mut job_metadata = Vec::with_capacity(args.proving.num_concurrent_preflights as usize);
    for i in 0..args.proving.num_concurrent_preflights as usize {
        job_metadata.push(PreflightJobMetadata {
            target_block: boundary_blocks[i + 1],
            agreed_l2_head_hash: blocks[i].header.hash,
            agreed_l2_output_root: outputs[i],
            claimed_l2_output_root: outputs[i + 1],
        });
    }

    // Now spawn all tasks without any RPC blocking
    let mut base_args = args.clone();
    base_args.proving.max_block_executions = usize::MAX;
    base_args.proving.max_block_derivations = u64::MAX;
    base_args.proving.max_witness_size = usize::MAX;

    // Spawn all tasks and collect into FuturesUnordered for as-completed processing
    let mut futures: FuturesUnordered<_> = job_metadata
        .into_iter()
        .map(|metadata| {
            let mut job_args = base_args.clone();
            job_args.kona.agreed_l2_head_hash = metadata.agreed_l2_head_hash;
            job_args.kona.agreed_l2_output_root = metadata.agreed_l2_output_root;
            job_args.kona.claimed_l2_block_number = metadata.target_block;
            job_args.kona.claimed_l2_output_root = metadata.claimed_l2_output_root;

            let target_block = metadata.target_block;
            let task = tokio::spawn(crate::tasks::compute_cached_proof(
                job_args,
                rollup_config.clone(),
                l1_config.clone(),
                disk_kv_store.clone(),
                Precondition::default(),
                B256::ZERO,
                vec![],
                None,
                None,
                vec![],
                vec![],
                vec![],
                false,
                true,
                false,
            ));

            // Wrap the task with its target block number
            async move { (target_block, task.await) }
        })
        .collect();

    // Process results as they complete (not in order)
    // This allows faster error detection and better resource utilization
    let mut l1_head_sufficient = true;
    while let Some((target_l2_height, join_result)) = futures.next().await {
        let result = join_result?;
        let claimed_l2_block_number = match result {
            Err(e) => {
                let ProvingError::NotSeekingProof(_, _, executions, ..) = e else {
                    error!("Error during preflight execution: {e:?}");
                    continue;
                };
                let Some(trace) = executions.first() else {
                    error!("L1 Head insufficient to derive L2 block beyond {target_l2_height}.");
                    l1_head_sufficient = false;
                    continue;
                };
                let Some(claimed_l2_block) = trace.last() else {
                    error!("L1 Head insufficient to derive L2 block beyond {target_l2_height}.");
                    l1_head_sufficient = false;
                    continue;
                };
                claimed_l2_block.artifacts.header.number
            }
            Ok((receipt, _)) => ProofJournal::from(&receipt.0).claimed_l2_block_number,
        };

        if claimed_l2_block_number < target_l2_height {
            error!("L1 Head insufficient to derive L2 block {target_l2_height}. Stopped at {claimed_l2_block_number}.");
            l1_head_sufficient = false;
        } else {
            info!("Preflight job for target {target_l2_height} terminated at {claimed_l2_block_number}.");
        };
    }

    Ok(l1_head_sufficient)
}

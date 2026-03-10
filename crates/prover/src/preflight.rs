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
use alloy::consensus::{Header, Transaction};
use alloy::eips::eip4844::{kzg_to_versioned_hash, IndexedBlobHash, FIELD_ELEMENTS_PER_BLOB};
use alloy::eips::{BlockId, BlockNumberOrTag};
use alloy::providers::{Provider, RootProvider};
use alloy_primitives::hex::FromHex;
use alloy_primitives::keccak256;
use alloy_primitives::B256;
use alloy_rlp::Decodable;
use anyhow::{anyhow, bail, Context};
use ark_ff::{BigInteger, PrimeField};
use kailua_kona::blobs::BlobFetchRequest;
use kailua_kona::journal::ProofJournal;
use kailua_kona::precondition::proposal::ProposalPrecondition;
use kailua_kona::precondition::Precondition;
use kailua_sync::provider::beacon::BlobProvider;
use kailua_sync::provider::optimism::OpNodeProvider;
use kailua_sync::{await_tel, retry_res_ctx, retry_res_ctx_timeout};
use kona_derive::L2ChainProvider;
use kona_genesis::{L1ChainConfig, RollupConfig};
use kona_host::single::SingleChainProviders;
use kona_host::KeyValueStore;
use kona_preimage::{PreimageKey, PreimageKeyType};
use kona_proof::l1::ROOTS_OF_UNITY;
use kona_protocol::BlockInfo;
use kona_providers_alloy::AlloyL2ChainProvider;
use opentelemetry::global::tracer;
use opentelemetry::trace::FutureExt;
use opentelemetry::trace::{TraceContextExt, Tracer};
use serde_json::Value;
use std::env::set_var;
use std::iter::zip;
use std::sync::Arc;
use tracing::{error, info, warn};

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

#[allow(clippy::too_many_arguments)]
pub async fn concurrent_preflight(
    args: &ProveArgs,
    rollup_config: RollupConfig,
    l1_config: L1ChainConfig,
    op_node_provider: &OpNodeProvider,
    disk_kv_store: Option<RWLKeyValueStore>,
) -> anyhow::Result<bool> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("concurrent_preflight"));

    // Create providers
    let SingleChainProviders {
        l1: l1_provider,
        l2: l2_provider,
        ..
    } = retry_res_ctx_timeout!(args.timeouts.max(), args.create_providers().await).await;
    let blob_provider = await_tel!(
        context,
        tracer,
        "BlobProvider::new",
        retry_res_ctx!(BlobProvider::new(
            args.kona
                .l1_beacon_address
                .clone()
                .ok_or_else(|| anyhow!("Missing beacon node address."))?,
            args.timeouts.beacon_rpc_timeout
        ))
    );

    // Resolve agreed L2 head block number (needed for both L1 origin derivation and L2 work)
    let starting_l2_block = await_tel!(
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
    .header;
    let mut num_l2_blocks = args.kona.claimed_l2_block_number - starting_l2_block.number;
    if num_l2_blocks == 0 {
        return Ok(true);
    }

    let rollup_config_arc = Arc::new(rollup_config.clone());
    let batcher_address = await_tel!(
        context,
        tracer,
        "sys_config",
        retry_res_ctx_timeout!(
            args.timeouts.op_geth_timeout,
            AlloyL2ChainProvider::new(l2_provider.clone(), rollup_config_arc.clone(), 1024)
                .system_config_by_number(starting_l2_block.number, rollup_config_arc.clone())
                .await
                .context("Failed to fetch system config")
        )
    )
    .batcher_address;

    // Determine l1_origin_number from OP Node
    let l1_origin_number = {
        let channel_timeout = rollup_config.channel_timeout(starting_l2_block.timestamp);
        let output: Value = await_tel!(
            context,
            tracer,
            "optimism_outputAtBlock for l1_origin",
            retry_res_ctx_timeout!(
                args.timeouts.op_node_timeout,
                op_node_provider
                    .0
                    .client()
                    .request::<(String,), Value>(
                        "optimism_outputAtBlock",
                        (format!("0x{:x}", starting_l2_block.number),),
                    )
                    .await
                    .context("optimism_outputAtBlock for l1_origin")
            )
        );
        output["blockRef"]["l1origin"]["number"]
            .as_u64()
            .map(|number| {
                number
                    .saturating_sub(channel_timeout)
                    .max(rollup_config.genesis.l1.number)
            })
    };

    // Pre-fetch L1 headers into disk KV store
    let mut l1_jobs = vec![];
    if let Some(l1_origin_number) = l1_origin_number {
        if let Some(ref disk_kv_store) = disk_kv_store {
            // 1. Get L1 head block number from the l1_head hash
            let l1_head_num = await_tel!(
                context,
                tracer,
                "l1_provider get_block_by_hash l1_head",
                retry_res_ctx_timeout!(
                    args.timeouts.eth_rpc_timeout,
                    l1_provider
                        .get_block_by_hash(args.kona.l1_head)
                        .await
                        .context("l1_provider get_block_by_hash l1_head")?
                        .ok_or_else(|| anyhow!("Failed to fetch L1 head block"))
                )
            )
            .header
            .number;

            // 2. Split range among num_concurrent_preflights worker tasks
            if l1_origin_number <= l1_head_num {
                let total_headers = l1_head_num - l1_origin_number + 1;
                let num_workers = args.proving.num_concurrent_preflights;
                let headers_per_worker = total_headers / num_workers;
                let mut extra = total_headers % num_workers;
                info!(
                    "Prefetching {total_headers} blob data from block {l1_origin_number} to {l1_head_num} with {num_workers} workers"
                );

                let mut start = l1_origin_number;
                for _ in 0..num_workers {
                    let chunk_size = if extra > 0 {
                        extra -= 1;
                        headers_per_worker + 1
                    } else {
                        headers_per_worker
                    };
                    let end = start + chunk_size - 1;
                    let l1_provider = l1_provider.clone();
                    let blob_provider = blob_provider.clone();
                    let kv = disk_kv_store.clone();
                    let timeout = args.timeouts.eth_rpc_timeout;
                    l1_jobs.push(tokio::spawn(async move {
                        let mut expected_hash: Option<B256> = None;
                        let mut expected_nonce: Option<u64> = None;
                        let mut blob_timestamp: Option<u64> = None;

                        for block_num in (start..=end).rev() {
                            let mut header = if let Some(hash) = expected_hash {
                                if let Some(cached) = kv.read().unwrap().get(hash) {
                                    // Cached — decode directly
                                    Some(
                                        Header::decode(&mut cached.as_slice())
                                            .context("Failed to RLP-decode cached L1 header")?,
                                    )
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            // query rpc
                            if header.is_none() {
                                let raw_header_hex: String = retry_res_ctx_timeout!(
                                    timeout,
                                    l1_provider
                                        .client()
                                        .request::<(BlockNumberOrTag,), String>(
                                            "debug_getRawHeader",
                                            (BlockNumberOrTag::Number(block_num),),
                                        )
                                        .await
                                        .context("debug_getRawHeader")
                                )
                                .await;
                                let raw_bytes = alloy_primitives::Bytes::from_hex(&raw_header_hex)?;
                                let hash = keccak256(raw_bytes.as_ref());
                                let key = PreimageKey::new_keccak256(*hash);

                                // Decode header to get parent_hash for next iteration
                                header = Some(
                                    Header::decode(&mut raw_bytes.as_ref())
                                        .context("Failed to RLP-decode L1 header")?,
                                );
                                kv.write().unwrap().set(key.into(), raw_bytes.into())?;
                            }
                            // set next header
                            let header = header.unwrap();
                            expected_hash = Some(header.parent_hash);
                            // skip if blobs preloaded
                            let inverse_header_hash = !header.hash_slow();
                            if kv.read().unwrap().get(inverse_header_hash).is_some() {
                                continue;
                            }
                            kv.write().unwrap().set(inverse_header_hash, vec![])?;
                            // check batcher's nonce at block height
                            let batcher_nonce = retry_res_ctx_timeout!(
                                timeout,
                                l1_provider
                                    .get_transaction_count(batcher_address,)
                                    .block_id(BlockId::Number(BlockNumberOrTag::Number(block_num)))
                                    .await
                                    .context("get_transaction_count")
                            )
                            .await;
                            // replace old expected nonce or skip if first block to process
                            let Some(expected_nonce) = expected_nonce.replace(batcher_nonce) else {
                                blob_timestamp = Some(header.timestamp);
                                continue;
                            };
                            let blob_timestamp = blob_timestamp.replace(header.timestamp).unwrap();

                            // nothing to do if no transactions were done
                            if batcher_nonce == expected_nonce {
                                info!("No transactions for {batcher_address} in {}", block_num + 1);
                                continue;
                            }

                            // fetch all slot blobs
                            let blobs = retry_res_ctx_timeout!(
                                blob_provider.timeout,
                                blob_provider
                                    .get_blobs(blob_provider.slot(blob_timestamp))
                                    .await
                            )
                            .await;

                            // save each blob to kv
                            let mut kv_lock = kv.write().unwrap();
                            for blob in blobs {
                                // Save this blob in the kv store
                                let versioned_hash =
                                    kzg_to_versioned_hash(blob.kzg_commitment.as_slice());

                                // Set the preimage for the blob commitment.
                                kv_lock.set(
                                    PreimageKey::new(*versioned_hash, PreimageKeyType::Sha256)
                                        .into(),
                                    blob.kzg_commitment.to_vec(),
                                )?;

                                // Write all the field elements to the key-value store. There should be 4096.
                                // The preimage oracle key for each field element is the keccak256 hash of
                                // `abi.encodePacked(sidecar.KZGCommitment, bytes32(ROOTS_OF_UNITY[i]))`.
                                let mut blob_key = [0u8; 80];
                                blob_key[..48].copy_from_slice(blob.kzg_commitment.as_ref());
                                for i in 0..FIELD_ELEMENTS_PER_BLOB {
                                    blob_key[48..].copy_from_slice(
                                        ROOTS_OF_UNITY[i as usize]
                                            .into_bigint()
                                            .to_bytes_be()
                                            .as_ref(),
                                    );
                                    let blob_key_hash = keccak256(blob_key.as_ref());

                                    kv_lock.set(
                                        PreimageKey::new_keccak256(*blob_key_hash).into(),
                                        blob_key.into(),
                                    )?;
                                    kv_lock.set(
                                        PreimageKey::new(*blob_key_hash, PreimageKeyType::Blob)
                                            .into(),
                                        blob.blob[(i as usize) << 5..(i as usize + 1) << 5]
                                            .to_vec(),
                                    )?;
                                }

                                // Write the KZG Proof as the 4096th element.
                                // Note: This is not associated with a root of unity, as to be backwards compatible
                                // with ZK users of kona that use this proof for the overall blob.
                                blob_key[72..].copy_from_slice(
                                    FIELD_ELEMENTS_PER_BLOB.to_be_bytes().as_ref(),
                                );
                                let blob_key_hash = keccak256(blob_key.as_ref());

                                kv_lock.set(
                                    PreimageKey::new_keccak256(*blob_key_hash).into(),
                                    blob_key.into(),
                                )?;
                                kv_lock.set(
                                    PreimageKey::new(*blob_key_hash, PreimageKeyType::Blob).into(),
                                    blob.kzg_proof.to_vec(),
                                )?;

                                info!(
                                    "Preloaded blob {versioned_hash} from block {}",
                                    block_num + 1
                                );
                            }
                        }
                        Ok::<(), anyhow::Error>(())
                    }));
                    start = end + 1;
                }
            }
        }
    }

    let blocks_per_thread = num_l2_blocks / args.proving.num_concurrent_preflights;
    let mut extra_blocks = num_l2_blocks % args.proving.num_concurrent_preflights;
    let mut jobs = vec![];
    let mut args = args.clone();
    args.proving.max_block_executions = usize::MAX;
    args.proving.max_block_derivations = u64::MAX;
    args.proving.max_witness_size = usize::MAX;
    while num_l2_blocks > 0 {
        let processed_blocks = if extra_blocks > 0 {
            extra_blocks -= 1;
            blocks_per_thread + 1
        } else {
            blocks_per_thread
        };
        num_l2_blocks = num_l2_blocks.saturating_sub(processed_blocks);

        // update ending block
        args.kona.claimed_l2_block_number = await_tel!(
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
        .number
            + processed_blocks;
        args.kona.claimed_l2_output_root = await_tel!(
            context,
            tracer,
            "output_at_block claimed_l2_block_number",
            retry_res_ctx_timeout!(
                args.timeouts.op_node_timeout,
                op_node_provider
                    .output_at_block(args.kona.claimed_l2_block_number)
                    .await
            )
        );
        // queue and start new job
        let task = tokio::spawn(crate::tasks::compute_cached_proof(
            args.clone(),
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
        jobs.push((args.kona.claimed_l2_block_number, task));
        // update starting block for next job
        if num_l2_blocks > 0 {
            args.kona.agreed_l2_head_hash = await_tel!(
                context,
                tracer,
                "l2_provider get_block_by_number claimed_l2_block_number",
                retry_res_ctx_timeout!(
                    args.timeouts.op_geth_timeout,
                    l2_provider
                        .get_block_by_number(BlockNumberOrTag::Number(
                            args.kona.claimed_l2_block_number
                        ))
                        .await
                        .context("l2_provider get_block_by_number claimed_l2_block_number")?
                        .ok_or_else(|| anyhow!("Failed to claimed l2 block"))
                )
            )
            .header
            .hash;

            args.kona.agreed_l2_output_root = args.kona.claimed_l2_output_root;
        }
    }
    // Await L1 header workers
    for job in l1_jobs {
        job.await??;
    }

    // Await L2 preflight tasks
    let mut l1_head_sufficient = true;
    for (target_l2_height, job) in jobs {
        let result = job.await?;
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

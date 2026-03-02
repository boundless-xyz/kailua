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
use alloy::consensus::Transaction;
use alloy::eips::eip2718::Encodable2718;
use alloy::eips::eip4844::{BlobTransactionSidecarItem, FIELD_ELEMENTS_PER_BLOB, IndexedBlobHash};
use alloy::eips::BlockNumberOrTag;
use alloy::providers::{Provider, RootProvider};
use alloy_primitives::{keccak256, Address, Bytes, B256};
use anyhow::{anyhow, Context};
use ark_ff::{BigInteger, PrimeField};
use futures::stream::{self, StreamExt};
use kailua_sync::retry_res_ctx_timeout;
use kona_genesis::RollupConfig;
use kona_host::KeyValueStore;
use kona_preimage::{PreimageKey, PreimageKeyType};
use kona_proof::l1::ROOTS_OF_UNITY;
use kona_protocol::{BlockInfo, L2BlockInfo};
use op_alloy_consensus::OpTxEnvelope;
use op_alloy_network::{Optimism, TransactionResponse};
use opentelemetry::trace::{TraceContextExt, Tracer};
use tracing::{info, warn};

/// A blob fetch task identified during L1 block scanning.
#[derive(Debug, Clone)]
struct BlobTask {
    /// The block timestamp (needed for beacon API).
    timestamp: u64,
    /// The versioned blob hashes from the transaction.
    blob_hashes: Vec<IndexedBlobHash>,
}

/// Aggressively prefetches all L1 data for the derivation range and stores it
/// in the disk KV store so that derivation reads only from cache.
pub async fn l1_prefetch(
    args: &ProveArgs,
    rollup_config: &RollupConfig,
    disk_kv_store: Option<RWLKeyValueStore>,
    l2_provider: Option<&RootProvider<Optimism>>,
) -> anyhow::Result<()> {
    if args.proving.skip_l1_prefetch {
        info!("Skipping L1 data prefetch (--skip-l1-prefetch).");
        return Ok(());
    }

    let Some(ref mut kv) = disk_kv_store.clone() else {
        warn!("No disk KV store available, skipping L1 prefetch.");
        return Ok(());
    };

    let Some(l2_provider) = l2_provider else {
        warn!("No L2 provider available, skipping L1 prefetch.");
        return Ok(());
    };

    let providers =
        retry_res_ctx_timeout!(args.timeouts.max(), args.create_providers().await).await;

    // Compute L1 block range
    let (l1_start, l1_end) = compute_l1_range(args, rollup_config, l2_provider).await?;

    if l1_start > l1_end {
        warn!("L1 prefetch range is empty (start={l1_start}, end={l1_end}), skipping.");
        return Ok(());
    }

    let num_blocks = l1_end - l1_start + 1;
    info!(
        "L1 prefetch: fetching {} blocks (L1 #{l1_start}..#{l1_end})",
        num_blocks
    );

    let batcher_address = rollup_config
        .genesis
        .system_config
        .as_ref()
        .map(|sc| sc.batcher_address)
        .unwrap_or_default();
    let batch_inbox_address = rollup_config.batch_inbox_address;

    // Phase 1: Fetch all L1 block headers, transactions, and receipts in parallel.
    // Collect blob tasks along the way.
    let block_numbers: Vec<u64> = (l1_start..=l1_end).collect();
    let l1_provider = &providers.l1;
    let blob_provider = &providers.blobs;
    let concurrency = args.proving.l1_prefetch_concurrency;
    let eth_rpc_timeout = args.timeouts.eth_rpc_timeout;

    let blob_tasks: Vec<BlobTask> = stream::iter(block_numbers)
        .map(|block_number| {
            let mut kv_clone = kv.clone();
            async move {
                fetch_and_store_l1_block(
                    l1_provider,
                    eth_rpc_timeout,
                    block_number,
                    &mut kv_clone,
                    batcher_address,
                    batch_inbox_address,
                )
                .await
            }
        })
        .buffer_unordered(concurrency)
        .filter_map(|result| async {
            match result {
                Ok(Some(task)) => Some(task),
                Ok(None) => None,
                Err(e) => {
                    warn!("L1 prefetch error for block: {e:?}");
                    None
                }
            }
        })
        .collect()
        .await;

    info!(
        "L1 prefetch: fetched {} blocks, found {} blocks with batcher blobs",
        num_blocks,
        blob_tasks.len()
    );

    // Phase 2: Fetch blobs in parallel
    if !blob_tasks.is_empty() {
        let blob_concurrency = args.proving.blob_prefetch_concurrency;
        let total_blobs: usize = blob_tasks.iter().map(|t| t.blob_hashes.len()).sum();
        info!(
            "L1 prefetch: fetching {total_blobs} blobs from {} blocks",
            blob_tasks.len()
        );

        // Flatten into individual blob fetch tasks
        let individual_blob_tasks: Vec<(u64, IndexedBlobHash)> = blob_tasks
            .into_iter()
            .flat_map(|task| {
                task.blob_hashes
                    .into_iter()
                    .map(move |hash| (task.timestamp, hash))
            })
            .collect();

        stream::iter(individual_blob_tasks)
            .map(|(timestamp, indexed_hash)| {
                let mut kv_clone = kv.clone();
                let versioned_hash = indexed_hash.hash;
                async move {
                    let partial_block_ref = BlockInfo {
                        timestamp,
                        ..Default::default()
                    };
                    let mut blobs = blob_provider
                        .fetch_filtered_blob_sidecars(&partial_block_ref, &[indexed_hash])
                        .await
                        .map_err(|e| anyhow!("Failed to fetch blob sidecar: {e}"))?;
                    if blobs.len() != 1 {
                        anyhow::bail!("Expected 1 blob, got {}", blobs.len());
                    }
                    let sidecar = blobs.pop().expect("Expected 1 blob");
                    store_blob_preimage(&mut kv_clone, versioned_hash, &sidecar)?;
                    Ok::<(), anyhow::Error>(())
                }
            })
            .buffer_unordered(blob_concurrency)
            .for_each(|result| async {
                if let Err(e) = result {
                    warn!("L1 blob prefetch error: {e:?}");
                }
            })
            .await;

        info!("L1 prefetch: blob fetching complete.");
    }

    info!("L1 prefetch complete.");
    Ok(())
}

/// Computes the L1 block range needed for derivation.
///
/// Start: `safe_head.l1_origin.number - channel_timeout`
/// End: `l1_head` block number
async fn compute_l1_range(
    args: &ProveArgs,
    rollup_config: &RollupConfig,
    l2_provider: &RootProvider<Optimism>,
) -> anyhow::Result<(u64, u64)> {
    let l1_end = {
        let providers =
            retry_res_ctx_timeout!(args.timeouts.max(), args.create_providers().await).await;
        let l1_head_block = retry_res_ctx_timeout!(
            args.timeouts.eth_rpc_timeout,
            providers
                .l1
                .get_block_by_hash(args.kona.l1_head)
                .await
                .context("get_block_by_hash l1_head")?
                .ok_or_else(|| anyhow!("Failed to fetch l1 head block"))
        )
        .await;
        l1_head_block.header.number
    };

    // Fetch the safe L2 head block to determine L1 origin
    let safe_head_block = retry_res_ctx_timeout!(
        args.timeouts.op_geth_timeout,
        l2_provider
            .get_block_by_hash(args.kona.agreed_l2_head_hash)
            .full()
            .await
            .context("get_block_by_hash agreed_l2_head")?
            .ok_or_else(|| anyhow!("Failed to fetch safe l2 head"))
    )
    .await;

    let safe_head_block = op_alloy_consensus::OpBlock {
        header: safe_head_block.header.into(),
        body: alloy::consensus::BlockBody {
            transactions: safe_head_block
                .transactions
                .as_transactions()
                .unwrap()
                .iter()
                .map(|t| {
                    let inner: &OpTxEnvelope = t.inner.inner.inner();
                    inner.clone()
                })
                .collect(),
            ommers: vec![],
            withdrawals: safe_head_block.withdrawals,
        },
    };

    let safe_head_info =
        L2BlockInfo::from_block_and_genesis(&safe_head_block, &rollup_config.genesis)
            .map_err(|e| anyhow!("{e}"))?;

    let channel_timeout = rollup_config.channel_timeout(safe_head_info.block_info.timestamp);
    let l1_start = safe_head_info
        .l1_origin
        .number
        .saturating_sub(channel_timeout)
        .max(rollup_config.genesis.l1.number);

    Ok((l1_start, l1_end))
}

/// Fetches and stores a single L1 block's header, transactions trie, and receipts trie.
/// Returns a `BlobTask` if the block contains batcher blob transactions.
async fn fetch_and_store_l1_block(
    l1_provider: &RootProvider,
    timeout: u64,
    block_number: u64,
    kv: &mut RWLKeyValueStore,
    batcher_address: Address,
    batch_inbox_address: Address,
) -> anyhow::Result<Option<BlobTask>> {
    // Fetch the full block (with transactions)
    let block = retry_res_ctx_timeout!(
        timeout,
        l1_provider
            .get_block_by_number(BlockNumberOrTag::Number(block_number))
            .full()
            .await
            .context(format!("get_block_by_number {block_number}"))?
            .ok_or_else(|| anyhow!("L1 block {block_number} not found"))
    )
    .await;

    let block_hash = block.header.hash;

    // 1. Store header: fetch raw RLP header via debug_getRawHeader
    let raw_header: Bytes = retry_res_ctx_timeout!(
        timeout,
        l1_provider
            .client()
            .request("debug_getRawHeader", [block_hash])
            .await
            .context(format!("debug_getRawHeader for block {block_number}"))
    )
    .await;
    kv.set(
        PreimageKey::new_keccak256(*block_hash).into(),
        raw_header.to_vec(),
    )?;

    // 2. Store transactions trie (L1 transactions: tx.inner gives TxEnvelope)
    let encoded_transactions: Vec<Vec<u8>> = block
        .transactions
        .as_transactions()
        .unwrap()
        .iter()
        .map(|tx| tx.inner.encoded_2718())
        .collect();
    store_ordered_trie(kv, &encoded_transactions)?;

    // 3. Store receipts trie: fetch raw receipts via debug_getRawReceipts
    let raw_receipts: Vec<Bytes> = retry_res_ctx_timeout!(
        timeout,
        l1_provider
            .client()
            .request("debug_getRawReceipts", [block_hash])
            .await
            .context(format!("debug_getRawReceipts for block {block_number}"))
    )
    .await;
    store_ordered_trie(kv, &raw_receipts)?;

    // 4. Detect batcher blob transactions
    let mut blob_hashes = Vec::new();
    let mut global_blob_index: u64 = 0;
    for tx in block.transactions.as_transactions().unwrap() {
        let tx_blob_hashes = tx.blob_versioned_hashes().unwrap_or_default();
        if tx.from() == batcher_address
            && tx.to() == Some(batch_inbox_address)
            && !tx_blob_hashes.is_empty()
        {
            for hash in tx_blob_hashes {
                blob_hashes.push(IndexedBlobHash {
                    index: global_blob_index,
                    hash: *hash,
                });
                global_blob_index += 1;
            }
        } else {
            global_blob_index += tx_blob_hashes.len() as u64;
        }
    }

    if blob_hashes.is_empty() {
        Ok(None)
    } else {
        Ok(Some(BlobTask {
            timestamp: block.header.timestamp,
            blob_hashes,
        }))
    }
}

/// Replicates kona-host's `store_ordered_trie` for synchronous KV store access.
///
/// Builds an ordered MPT from the given values and stores all intermediate trie
/// nodes in the KV store keyed by `keccak256(node) -> node`.
fn store_ordered_trie<T: AsRef<[u8]>>(
    kv: &mut RWLKeyValueStore,
    values: &[T],
) -> anyhow::Result<()> {
    use alloy::consensus::EMPTY_ROOT_HASH;

    if values.is_empty() {
        let empty_key = PreimageKey::new(*EMPTY_ROOT_HASH, PreimageKeyType::Keccak256);
        // EMPTY_STRING_CODE = 0x80 (RLP encoding of empty string)
        return kv.set(empty_key.into(), vec![0x80]);
    }

    let mut hb = kona_mpt::ordered_trie_with_encoder(values, |node, buf| {
        buf.put_slice(node.as_ref());
    });
    hb.root();
    let intermediates = hb.take_proof_nodes().into_inner();

    for (_, value) in intermediates.into_iter() {
        let value_hash = keccak256(value.as_ref());
        let key = PreimageKey::new(*value_hash, PreimageKeyType::Keccak256);
        kv.set(key.into(), value.into())?;
    }

    Ok(())
}

/// Stores a blob's preimage data in the exact format expected by kona-host.
///
/// This stores:
/// - The commitment keyed by SHA256 of the versioned hash
/// - 4096 field elements, each keyed by keccak256(commitment || ROOTS_OF_UNITY[i])
/// - The KZG proof as the 4096th element
fn store_blob_preimage(
    kv: &mut RWLKeyValueStore,
    versioned_hash: B256,
    sidecar: &BlobTransactionSidecarItem,
) -> anyhow::Result<()> {
    let BlobTransactionSidecarItem {
        blob,
        kzg_proof: proof,
        kzg_commitment: commitment,
        ..
    } = sidecar;

    // Store the commitment keyed by versioned hash (SHA256 key type)
    kv.set(
        PreimageKey::new(*versioned_hash, PreimageKeyType::Sha256).into(),
        commitment.to_vec(),
    )?;

    // Write all 4096 field elements
    // Key = abi.encodePacked(commitment, bytes32(ROOTS_OF_UNITY[i]))
    let mut blob_key = [0u8; 80];
    blob_key[..48].copy_from_slice(commitment.as_ref());
    for i in 0..FIELD_ELEMENTS_PER_BLOB {
        blob_key[48..].copy_from_slice(
            ROOTS_OF_UNITY[i as usize]
                .into_bigint()
                .to_bytes_be()
                .as_ref(),
        );
        let blob_key_hash = keccak256(blob_key.as_ref());

        kv.set(
            PreimageKey::new_keccak256(*blob_key_hash).into(),
            blob_key.into(),
        )?;
        kv.set(
            PreimageKey::new(*blob_key_hash, PreimageKeyType::Blob).into(),
            blob[(i as usize) << 5..(i as usize + 1) << 5].to_vec(),
        )?;
    }

    // Write the KZG proof as the 4096th element
    blob_key[72..].copy_from_slice(FIELD_ELEMENTS_PER_BLOB.to_be_bytes().as_ref());
    let blob_key_hash = keccak256(blob_key.as_ref());
    kv.set(
        PreimageKey::new_keccak256(*blob_key_hash).into(),
        blob_key.into(),
    )?;
    kv.set(
        PreimageKey::new(*blob_key_hash, PreimageKeyType::Blob).into(),
        proof.to_vec(),
    )?;

    Ok(())
}

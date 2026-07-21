// Copyright 2025 Boundless Foundation, Inc.
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

use crate::kv::{create_split_kv_store, RWLKeyValueStore};
use crate::ProvingError;
use alloy::eips::BlockNumberOrTag;
use alloy::providers::{Provider, RootProvider};
use alloy_primitives::hex::FromHex;
use alloy_primitives::keccak256;
use anyhow::anyhow;
use human_bytes::human_bytes;
use kailua_sync::provider::optimism::OpNodeProvider;
use kona_host::KeyValueStore;
use kona_preimage::{PreimageKey, PreimageKeyType};
use kona_proof::BootInfo;
use std::ops::DerefMut;
use tracing::{error, info};

/// Preloads execution-witness preimages for every block in the claim into the KV store.
///
/// Walks backwards from the claimed block to the agreed block, querying `debug_executionWitness`
/// (retrying indefinitely) and hashing every hex string in the response into the store as a
/// keccak preimage. Processed blocks are marked with a global-generic key to avoid rework.
pub async fn run_payload_client(
    mut boot_info: BootInfo,
    l2_provider: RootProvider,
    op_node_provider: OpNodeProvider,
    disk_kv_store: Option<RWLKeyValueStore>,
) -> anyhow::Result<bool> {
    let kv = create_split_kv_store(&Default::default(), disk_kv_store)
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;

    while boot_info.claimed_l2_output_root != boot_info.agreed_l2_output_root {
        // Go back one block
        boot_info.claimed_l2_block_number -= 1;
        boot_info.claimed_l2_output_root = op_node_provider
            .output_at_block(boot_info.claimed_l2_block_number)
            .await?;

        // Check if block payload had already been processed
        let kv_lock = kv.read().await;
        // we insert a special marker using the inverted output root as a global generic
        let exec_wit_key = PreimageKey::new(
            (!boot_info.claimed_l2_output_root).0,
            PreimageKeyType::GlobalGeneric,
        );
        if kv_lock.get(exec_wit_key.into()).is_some() {
            info!(
                "Payload for {} already processed.",
                boot_info.claimed_l2_block_number + 1
            );
            continue;
        }
        drop(kv_lock);

        // retry endpoint indefinitely
        let execution_witness = loop {
            match l2_provider
                .client()
                .request::<(BlockNumberOrTag,), serde_json::Value>(
                    "debug_executionWitness",
                    (BlockNumberOrTag::Number(
                        boot_info.claimed_l2_block_number + 1,
                    ),),
                )
                .await
            {
                Ok(witness) => break witness,
                Err(err) => {
                    // Give up only when the method is intentionally blocked via
                    // --blocked-rpc-methods: that can never succeed. A generic "method not
                    // found" may recover on retry across a load-balanced provider pool, so we
                    // keep retrying as before.
                    let err = anyhow!(err);
                    if crate::hint_backoff::is_method_blacklisted(&err) {
                        error!(
                            "debug_executionWitness blocked, skipping payload preflight: {err:#}"
                        );
                        return Ok(false);
                    }
                    error!(
                        "Failed to fetch payload for {} (Retry)\n{err:?}.",
                        boot_info.claimed_l2_block_number + 1,
                    )
                }
            };
        };

        // dump preimages into kv store
        let mut kv_lock = kv.write().await;
        let payload_size = dump_payload_to_kv_store(&execution_witness, kv_lock.deref_mut());
        // Mark block payload as processed in kv store
        kv_lock.set(exec_wit_key.into(), vec![])?;
        info!(
            "Saved {} payload for {}.",
            human_bytes(payload_size as f64),
            boot_info.claimed_l2_block_number + 1
        );
        drop(kv_lock);
    }

    Ok(true)
}

/// Recursively saves every hex string in the JSON value as a keccak preimage, returning the
/// total bytes saved.
fn dump_payload_to_kv_store(payload: &serde_json::Value, kv: &mut dyn KeyValueStore) -> u64 {
    if let Some(obj) = payload.as_object() {
        obj.iter()
            .map(|(k, v)| save_hex_preimage_to_kv(k, kv) + dump_payload_to_kv_store(v, kv))
            .sum()
    } else if let Some(seq) = payload.as_array() {
        seq.iter().map(|v| dump_payload_to_kv_store(v, kv)).sum()
    } else if let Some(v) = payload.as_str() {
        save_hex_preimage_to_kv(v, kv)
    } else {
        0
    }
}

fn save_hex_preimage_to_kv(preimage: &str, kv: &mut dyn KeyValueStore) -> u64 {
    alloy_primitives::Bytes::from_hex(preimage)
        .map(|preimage| {
            let computed_hash = keccak256(preimage.as_ref());
            let key = PreimageKey::new_keccak256(*computed_hash);
            let size = preimage.len() as u64;
            kv.set(key.into(), preimage.into()).unwrap();
            size
        })
        .unwrap_or(0)
}

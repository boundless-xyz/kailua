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

//! Fallback hint handler that wraps [`SingleChainHintHandler`] with support for
//! the standard `/eth/v1/beacon/blob_sidecars/{slot}` endpoint when the Fusaka-era
//! `/eth/v1/beacon/blobs/{slot}` endpoint is unavailable.

use alloy::eips::eip4844::{kzg_to_versioned_hash, FIELD_ELEMENTS_PER_BLOB};
use alloy_primitives::keccak256;
use alloy_primitives::B256;
use alloy_rpc_types_beacon::sidecar::BeaconBlobBundle;
use anyhow::{anyhow, Result};
use ark_ff::{BigInteger, PrimeField};
use async_trait::async_trait;
use kona_host::single::{SingleChainHintHandler, SingleChainHost, SingleChainProviders};
use kona_host::{HintHandler, OnlineHostBackendCfg, SharedKeyValueStore};
use kona_preimage::{PreimageKey, PreimageKeyType};
use kona_proof::l1::ROOTS_OF_UNITY;
use kona_proof::{Hint, HintType};
use reqwest::Client;
use tracing::warn;

/// Parses a blob hint from raw bytes. Supports two formats:
/// - 40 bytes (new): hash (32) + timestamp (8)
/// - 48 bytes (legacy): hash (32) + index (8) + timestamp (8)
fn parse_blob_hint(data: &[u8]) -> Result<(B256, u64)> {
    match data.len() {
        40 => {
            let hash = B256::from_slice(&data[..32]);
            let timestamp = u64::from_be_bytes(data[32..40].try_into().unwrap());
            Ok((hash, timestamp))
        }
        48 => {
            let hash = B256::from_slice(&data[..32]);
            // bytes 32..40 are the blob index (unused)
            let timestamp = u64::from_be_bytes(data[40..48].try_into().unwrap());
            Ok((hash, timestamp))
        }
        n => Err(anyhow!("Invalid blob hint length: {n} (expected 40 or 48)")),
    }
}

/// A hint handler that wraps [`SingleChainHintHandler`] with a fallback for blob fetching.
///
/// kona-host v1.2.12 uses the Fusaka-era `/eth/v1/beacon/blobs/{slot}` endpoint exclusively,
/// which is not yet supported by current consensus layer clients (Lighthouse, Teku, etc.).
/// This handler intercepts `L1Blob` hints and falls back to the standard
/// `/eth/v1/beacon/blob_sidecars/{slot}` endpoint when the Fusaka endpoint fails.
#[derive(Debug, Clone, Copy)]
pub struct FallbackBlobHintHandler;

#[async_trait]
impl HintHandler for FallbackBlobHintHandler {
    type Cfg = SingleChainHost;

    async fn fetch_hint(
        hint: Hint<<Self::Cfg as OnlineHostBackendCfg>::HintType>,
        cfg: &Self::Cfg,
        providers: &<Self::Cfg as OnlineHostBackendCfg>::Providers,
        kv: SharedKeyValueStore,
    ) -> Result<()> {
        if hint.ty != HintType::L1Blob {
            return SingleChainHintHandler::fetch_hint(hint, cfg, providers, kv).await;
        }

        // Save data before passing hint (consumed by value)
        let hint_data = hint.data.clone();

        // Try the standard handler first (uses Fusaka /blobs/ endpoint)
        let result = SingleChainHintHandler::fetch_hint(
            Hint {
                ty: HintType::L1Blob,
                data: hint.data,
            },
            cfg,
            providers,
            kv.clone(),
        )
        .await;

        if result.is_ok() {
            return result;
        }

        warn!("Fusaka blob endpoint failed, falling back to blob_sidecars endpoint");
        Self::fetch_blob_fallback(&hint_data, cfg, providers, kv).await
    }
}

impl FallbackBlobHintHandler {
    /// Fetches blob data using the standard `/eth/v1/beacon/blob_sidecars/{slot}` endpoint
    /// and writes the preimage oracle key-value entries in the same format as
    /// [`SingleChainHintHandler`].
    async fn fetch_blob_fallback(
        hint_data: &[u8],
        cfg: &SingleChainHost,
        providers: &SingleChainProviders,
        kv: SharedKeyValueStore,
    ) -> Result<()> {
        let (hash, timestamp) = parse_blob_hint(hint_data)?;

        // Compute slot from timestamp
        let genesis_time = providers.blobs.genesis_time;
        let slot_interval = providers.blobs.slot_interval;
        let slot = timestamp
            .checked_sub(genesis_time)
            .ok_or_else(|| anyhow!("Timestamp {timestamp} is before genesis {genesis_time}"))?
            / slot_interval;

        // Fetch from /blob_sidecars endpoint
        let beacon_url = cfg
            .l1_beacon_address
            .as_ref()
            .ok_or_else(|| anyhow!("Beacon API URL not set"))?
            .trim_end_matches('/');

        let client = Client::new();
        let response: BeaconBlobBundle = client
            .get(format!("{beacon_url}/eth/v1/beacon/blob_sidecars/{slot}"))
            .send()
            .await
            .map_err(|e| anyhow!("Failed to fetch blob sidecars: {e}"))?
            .error_for_status()
            .map_err(|e| anyhow!("Failed to fetch blob sidecars: {e}"))?
            .json()
            .await
            .map_err(|e| anyhow!("Failed to parse blob sidecars response: {e}"))?;

        // Find the matching blob by versioned hash
        let blob_data = response
            .data
            .into_iter()
            .find(|b| kzg_to_versioned_hash(b.kzg_commitment.as_slice()) == hash)
            .ok_or_else(|| anyhow!("Blob with hash {hash} not found in slot {slot}"))?;

        let blob = blob_data.blob;
        let commitment = blob_data.kzg_commitment;
        let proof = blob_data.kzg_proof;

        // Write kv entries in the same format as SingleChainHintHandler
        let mut kv_lock = kv.write().await;

        // Set the preimage for the blob commitment
        kv_lock.set(
            PreimageKey::new(*hash, PreimageKeyType::Sha256).into(),
            commitment.to_vec(),
        )?;

        // Write all field elements to the key-value store
        let mut blob_key = [0u8; 80];
        blob_key[..48].copy_from_slice(commitment.as_slice());
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
                PreimageKey::new(*blob_key_hash, PreimageKeyType::Blob).into(),
                blob[(i as usize) << 5..(i as usize + 1) << 5].to_vec(),
            )?;
        }

        // Write the KZG proof as the final element
        blob_key[72..].copy_from_slice(FIELD_ELEMENTS_PER_BLOB.to_be_bytes().as_ref());
        let blob_key_hash = keccak256(blob_key.as_ref());
        kv_lock.set(
            PreimageKey::new_keccak256(*blob_key_hash).into(),
            blob_key.into(),
        )?;
        kv_lock.set(
            PreimageKey::new(*blob_key_hash, PreimageKeyType::Blob).into(),
            proof.to_vec(),
        )?;

        Ok(())
    }
}

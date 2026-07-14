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

//! Generic hint-handler wrapper that adds a fallback from the Fusaka-era
//! `/eth/v1/beacon/blobs/{slot}` endpoint to the standard
//! `/eth/v1/beacon/blob_sidecars/{slot}` endpoint for L1 blob hints.

use alloy::eips::eip4844::{
    kzg_to_versioned_hash, BlobTransactionSidecarItem, IndexedBlobHash, FIELD_ELEMENTS_PER_BLOB,
};
use alloy_primitives::keccak256;
use alloy_primitives::B256;
use alloy_rpc_types_beacon::sidecar::BeaconBlobBundle;
use anyhow::{anyhow, ensure, Result};
use ark_ff::{BigInteger, PrimeField};
use async_trait::async_trait;
use kona_host::single::{SingleChainHintHandler, SingleChainHost, SingleChainProviders};
use kona_host::{HintHandler, OnlineHostBackendCfg, SharedKeyValueStore};
use kona_preimage::{PreimageKey, PreimageKeyType};
use kona_proof::l1::ROOTS_OF_UNITY;
use kona_proof::{Hint, HintType};
use reqwest::Client;
use std::marker::PhantomData;
use tracing::warn;

#[derive(Debug, Clone, Copy)]
struct ParsedBlobHint {
    hash: B256,
    index: Option<u64>,
    timestamp: u64,
}

/// Parses a blob hint from raw bytes. Supports two formats:
/// - 40 bytes (new): hash (32) + timestamp (8)
/// - 48 bytes (legacy): hash (32) + index (8) + timestamp (8)
fn parse_blob_hint(data: &[u8]) -> Result<ParsedBlobHint> {
    match data.len() {
        40 => {
            let hash = B256::from_slice(&data[..32]);
            let timestamp = u64::from_be_bytes(data[32..40].try_into().unwrap());
            Ok(ParsedBlobHint {
                hash,
                index: None,
                timestamp,
            })
        }
        48 => {
            let hash = B256::from_slice(&data[..32]);
            let index = u64::from_be_bytes(data[32..40].try_into().unwrap());
            let timestamp = u64::from_be_bytes(data[40..48].try_into().unwrap());
            Ok(ParsedBlobHint {
                hash,
                index: Some(index),
                timestamp,
            })
        }
        n => Err(anyhow!("Invalid blob hint length: {n} (expected 40 or 48)")),
    }
}

/// Adapter for hint-handler configurations that can expose the underlying single-chain beacon
/// blob configuration required by the fallback path.
pub trait BlobFallbackAdapter: OnlineHostBackendCfg {
    /// Returns true when the outer hint type represents a standard L1 blob hint.
    fn is_l1_blob_hint(ty: &Self::HintType) -> bool;

    /// Returns the underlying single-chain config used for beacon blob fetching.
    fn single_chain_cfg(cfg: &Self) -> &SingleChainHost;

    /// Returns the underlying single-chain providers used for beacon blob fetching.
    fn single_chain_providers(providers: &Self::Providers) -> &SingleChainProviders;
}

/// Generic hint-handler wrapper that retries standard L1 blob hints via
/// `/eth/v1/beacon/blob_sidecars/{slot}` when the inner handler fails on the newer
/// `/eth/v1/beacon/blobs/{slot}` endpoint.
#[derive(Debug, Clone, Copy)]
pub struct BlobFallbackWrapper<Inner, Cfg>(PhantomData<(Inner, Cfg)>);

impl<Inner, Cfg> Default for BlobFallbackWrapper<Inner, Cfg> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

#[async_trait]
impl<Inner, Cfg> HintHandler for BlobFallbackWrapper<Inner, Cfg>
where
    Inner: HintHandler<Cfg = Cfg> + Send + Sync + 'static,
    Cfg: BlobFallbackAdapter + OnlineHostBackendCfg + Send + Sync + 'static,
{
    type Cfg = Cfg;

    async fn fetch_hint(
        hint: Hint<<Self::Cfg as OnlineHostBackendCfg>::HintType>,
        cfg: &Self::Cfg,
        providers: &<Self::Cfg as OnlineHostBackendCfg>::Providers,
        kv: SharedKeyValueStore,
    ) -> Result<()> {
        if !Cfg::is_l1_blob_hint(&hint.ty) {
            return Inner::fetch_hint(hint, cfg, providers, kv).await;
        }

        // Save data before passing hint (consumed by value)
        let hint_data = hint.data.clone();

        // Try the inner handler first (which may use the Fusaka /blobs/ endpoint).
        match Inner::fetch_hint(hint, cfg, providers, kv.clone()).await {
            Ok(()) => Ok(()),
            Err(err) => {
                warn!(
                    error = %err,
                    inner_handler = std::any::type_name::<Inner>(),
                    "Fusaka blob endpoint failed, falling back to blob_sidecars endpoint"
                );
                fetch_blob_fallback(
                    &hint_data,
                    Cfg::single_chain_cfg(cfg),
                    Cfg::single_chain_providers(providers),
                    kv,
                )
                .await
            }
        }
    }
}

impl BlobFallbackAdapter for SingleChainHost {
    fn is_l1_blob_hint(ty: &Self::HintType) -> bool {
        *ty == HintType::L1Blob
    }

    fn single_chain_cfg(cfg: &Self) -> &SingleChainHost {
        cfg
    }

    fn single_chain_providers(providers: &Self::Providers) -> &SingleChainProviders {
        providers
    }
}

#[cfg(feature = "eigen")]
impl BlobFallbackAdapter for hokulea_host_bin::cfg::SingleChainHostWithEigenDA {
    fn is_l1_blob_hint(ty: &Self::HintType) -> bool {
        matches!(
            ty,
            hokulea_proof::hint::ExtendedHintType::Original(HintType::L1Blob)
        )
    }

    fn single_chain_cfg(cfg: &Self) -> &SingleChainHost {
        &cfg.kona_cfg
    }

    fn single_chain_providers(providers: &Self::Providers) -> &SingleChainProviders {
        &providers.kona_providers
    }
}

#[cfg(feature = "celestia")]
impl BlobFallbackAdapter for hana_host::celestia::CelestiaChainHost {
    fn is_l1_blob_hint(ty: &Self::HintType) -> bool {
        matches!(
            ty,
            hana_oracle::hint::HintWrapper::Standard(HintType::L1Blob)
        )
    }

    fn single_chain_cfg(cfg: &Self) -> &SingleChainHost {
        &cfg.single_host
    }

    fn single_chain_providers(providers: &Self::Providers) -> &SingleChainProviders {
        &providers.inner_providers
    }
}

pub type FallbackBlobHintHandler = BlobFallbackWrapper<SingleChainHintHandler, SingleChainHost>;

#[cfg(feature = "eigen")]
pub type FallbackBlobHintHandlerWithEigenDA = BlobFallbackWrapper<
    hokulea_host_bin::handler::SingleChainHintHandlerWithEigenDA,
    hokulea_host_bin::cfg::SingleChainHostWithEigenDA,
>;

#[cfg(feature = "celestia")]
pub type FallbackHanaHintHandler = BlobFallbackWrapper<
    crate::hana::handler::HanaHintHandler,
    hana_host::celestia::CelestiaChainHost,
>;

fn validate_blob_sidecar(
    sidecar: &BlobTransactionSidecarItem,
    hash: B256,
    hinted_index: Option<u64>,
) -> Result<()> {
    if let Some(index) = hinted_index {
        sidecar
            .verify_blob(&IndexedBlobHash { index, hash })
            .map_err(|err| anyhow!("Blob sidecar validation failed: {err}"))?;
        return Ok(());
    }

    let computed_hash = B256::from(sidecar.to_kzg_versioned_hash());
    ensure!(
        computed_hash == hash,
        "Blob sidecar validation failed: expected versioned hash {hash}, got {computed_hash}",
    );
    sidecar
        .verify_blob_kzg_proof()
        .map_err(|err| anyhow!("Blob sidecar validation failed: {err}"))?;

    Ok(())
}

/// Fetches blob data using the standard `/eth/v1/beacon/blob_sidecars/{slot}` endpoint and writes
/// the preimage oracle key-value entries in the same format as [`SingleChainHintHandler`].
async fn fetch_blob_fallback(
    hint_data: &[u8],
    cfg: &SingleChainHost,
    providers: &SingleChainProviders,
    kv: SharedKeyValueStore,
) -> Result<()> {
    let ParsedBlobHint {
        hash,
        index,
        timestamp,
    } = parse_blob_hint(hint_data)?;

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

    // Find and validate the matching blob sidecar before writing any oracle entries.
    let mut validation_error = None;
    let mut matching_sidecar = None;
    for blob_data in response.data {
        if kzg_to_versioned_hash(blob_data.kzg_commitment.as_slice()) != hash {
            continue;
        }

        let sidecar = BlobTransactionSidecarItem {
            index: blob_data.index,
            blob: blob_data.blob,
            kzg_commitment: blob_data.kzg_commitment,
            kzg_proof: blob_data.kzg_proof,
        };

        match validate_blob_sidecar(&sidecar, hash, index) {
            Ok(()) => {
                matching_sidecar = Some(sidecar);
                break;
            }
            Err(err) => validation_error = Some(err),
        }
    }

    let BlobTransactionSidecarItem {
        blob,
        kzg_commitment: commitment,
        kzg_proof: proof,
        ..
    } = match matching_sidecar {
        Some(sidecar) => sidecar,
        None => {
            return Err(validation_error
                .unwrap_or_else(|| anyhow!("Blob with hash {hash} not found in slot {slot}")));
        }
    };

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
            blob[i as usize * 32..(i as usize + 1) * 32].to_vec(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::eips::eip4844::{env_settings::EnvKzgSettings, Blob, BlobTransactionSidecar};

    fn sample_sidecar(index: u64) -> (BlobTransactionSidecarItem, B256) {
        let mut sidecar = BlobTransactionSidecar::try_from_blobs_with_settings(
            vec![Blob::default()],
            EnvKzgSettings::Default.get(),
        )
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
        sidecar.index = index;
        let hash = B256::from(sidecar.to_kzg_versioned_hash());
        (sidecar, hash)
    }

    #[test]
    fn parse_blob_hint_new_format() {
        let hash = B256::repeat_byte(0xAB);
        let timestamp = 9999u64;

        let mut hint = [0u8; 40];
        hint[..32].copy_from_slice(hash.as_slice());
        hint[32..40].copy_from_slice(&timestamp.to_be_bytes());

        let parsed = parse_blob_hint(&hint).unwrap();
        assert_eq!(parsed.hash, hash);
        assert_eq!(parsed.index, None);
        assert_eq!(parsed.timestamp, timestamp);
    }

    #[test]
    fn parse_blob_hint_preserves_legacy_index() {
        let hash = B256::repeat_byte(0x42);
        let index = 7u64;
        let timestamp = 1234u64;

        let mut hint = [0u8; 48];
        hint[..32].copy_from_slice(hash.as_slice());
        hint[32..40].copy_from_slice(&index.to_be_bytes());
        hint[40..48].copy_from_slice(&timestamp.to_be_bytes());

        let parsed = parse_blob_hint(&hint).unwrap();
        assert_eq!(parsed.hash, hash);
        assert_eq!(parsed.index, Some(index));
        assert_eq!(parsed.timestamp, timestamp);
    }

    #[test]
    fn validate_blob_sidecar_checks_legacy_indexed_hints() {
        let (sidecar, hash) = sample_sidecar(3);
        validate_blob_sidecar(&sidecar, hash, Some(3)).unwrap();

        let err = validate_blob_sidecar(&sidecar, hash, Some(4)).unwrap_err();
        assert!(err.to_string().contains("Blob sidecar validation failed"));
    }

    #[test]
    fn validate_blob_sidecar_checks_hash_and_proof_without_index() {
        let (sidecar, hash) = sample_sidecar(9);
        validate_blob_sidecar(&sidecar, hash, None).unwrap();

        let mut invalid_proof = sidecar.clone();
        invalid_proof.kzg_proof = [0u8; 48].into();
        let err = validate_blob_sidecar(&invalid_proof, hash, None).unwrap_err();
        assert!(err.to_string().contains("Blob sidecar validation failed"));
    }
}

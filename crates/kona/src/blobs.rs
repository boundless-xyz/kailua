// Copyright 2024 Boundless Foundation, Inc.
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
use crate::rkyv::kzg::{BlobDef, Bytes48Def};
use alloy_eips::eip4844::{
    kzg_to_versioned_hash, Blob, IndexedBlobHash, BLS_MODULUS, FIELD_ELEMENTS_PER_BLOB,
};
use alloy_primitives::{B256, U256};
use anyhow::bail;
use async_trait::async_trait;
use c_kzg::{ethereum_kzg_settings, Bytes48};
use kona_derive::BlobProvider;
use kona_derive::BlobProviderError;
use kona_protocol::BlockInfo;
use serde::{Deserialize, Serialize};

/// Identifies a blob to fetch by its versioned hash, slot index, and publishing L1 block.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BlobFetchRequest {
    /// Contains the block height, hash, timestamp, and parent hash.
    pub block_ref: BlockInfo,
    /// Represents the versioned hash of a blob, and its index in the slot.
    pub blob_hash: IndexedBlobHash,
}

/// Host-supplied blobs with their KZG commitments and proofs, untrusted until verified through
/// conversion into a [PreloadedBlobProvider].
#[derive(
    Clone,
    Debug,
    Default,
    Eq,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub struct BlobWitnessData {
    /// A vector of `Blob` instances.
    #[rkyv(with = rkyv::with::Map<BlobDef>)]
    pub blobs: Vec<Blob>,
    /// A vector of `Bytes48` elements representing KZG commitments for each blob instance.
    #[rkyv(with = rkyv::with::Map<Bytes48Def>)]
    pub commitments: Vec<Bytes48>,
    /// A vector of `Bytes48` instances representing KZG blob proofs for each blob instance.
    #[rkyv(with = rkyv::with::Map<Bytes48Def>)]
    pub proofs: Vec<Bytes48>,
}

impl<T: Into<Blob>> From<Vec<T>> for BlobWitnessData {
    /// Computes the KZG commitment and proof for each blob, panicking on failure.
    fn from(blobs: Vec<T>) -> Self {
        let mut result = Self::default();
        let settings_ref = ethereum_kzg_settings(0);
        for blob in blobs {
            let blob: Blob = blob.into();
            let c_kzg_blob = c_kzg::Blob::new(blob.0);
            let commitment = settings_ref
                .blob_to_kzg_commitment(&c_kzg_blob)
                .expect("Failed to convert blob to commitment");
            let proof = settings_ref
                .compute_blob_kzg_proof(&c_kzg_blob, &commitment.to_bytes())
                .unwrap();
            // save values
            result.blobs.push(Blob::from(*c_kzg_blob));
            result.commitments.push(commitment.to_bytes());
            result.proofs.push(proof.to_bytes());
        }
        result
    }
}

/// [BlobProvider] serving KZG-verified blobs preloaded from a witness, in expected fetch order.
#[derive(Clone, Debug, Default)]
pub struct PreloadedBlobProvider {
    /// `(versioned hash, blob)` pairs, in reverse of the expected fetch order.
    entries: Vec<(B256, Blob)>,
}

impl From<BlobWitnessData> for PreloadedBlobProvider {
    /// Batch-verifies every blob's KZG proof, panicking on failure, then indexes each blob by
    /// the versioned hash of its commitment, reversed for consumption by pops.
    fn from(value: BlobWitnessData) -> Self {
        let blobs = value
            .blobs
            .into_iter()
            .map(|b| c_kzg::Blob::new(b.0))
            .collect::<Vec<_>>();
        assert!(
            ethereum_kzg_settings(0)
                .verify_blob_kzg_proof_batch(
                    blobs.as_slice(),
                    value.commitments.as_slice(),
                    value.proofs.as_slice(),
                )
                .expect("Failed to batch validate kzg proofs"),
            "Blob KZG proof batch verification failed"
        );

        let hashes = value
            .commitments
            .iter()
            .map(|c| kzg_to_versioned_hash(c.as_slice()))
            .collect::<Vec<_>>();
        let entries = core::iter::zip(hashes, blobs.into_iter().map(|b| Blob::from(*b)))
            .rev()
            .collect::<Vec<_>>();
        Self { entries }
    }
}

#[async_trait]
impl BlobProvider for PreloadedBlobProvider {
    type Error = BlobProviderError;

    /// Serves each requested hash by popping the next preloaded entry, erring on any mismatch.
    /// The block reference goes unused: requested hashes originate from authenticated L1 data,
    /// so the hash comparison alone authenticates the returned blobs.
    async fn get_and_validate_blobs(
        &mut self,
        _block_ref: &BlockInfo,
        blob_hashes: &[B256],
    ) -> Result<Vec<Box<Blob>>, Self::Error> {
        let blob_count = blob_hashes.len();
        log(&format!("FETCH {blob_count} BLOB(S)"));
        let mut blobs = Vec::with_capacity(blob_count);
        for hash in blob_hashes {
            let (blob_hash, blob) = self.entries.pop().unwrap();
            if *hash != blob_hash {
                return Err(BlobProviderError::Backend(format!(
                    "Expected entry with hash {hash} but found {blob_hash}",
                )));
            }
            blobs.push(Box::new(blob));
        }
        Ok(blobs)
    }
}

/// Extracts the first `blocks` field elements of the blob: the intermediate output roots that a
/// proposal publishes.
pub fn intermediate_outputs(blob: impl AsRef<Blob>, blocks: usize) -> anyhow::Result<Vec<U256>> {
    field_elements(blob, 0..blocks)
}

/// Extracts the field elements after the first `blocks` ones: the trailing blob data expected
/// to be all zeros in a well-formed proposal.
pub fn trail_data(blob: impl AsRef<Blob>, blocks: usize) -> anyhow::Result<Vec<U256>> {
    field_elements(blob, blocks..FIELD_ELEMENTS_PER_BLOB as usize)
}

/// Extracts the 32-byte big-endian field elements at the given indices, erring on any value not
/// below the BLS modulus.
pub fn field_elements(
    blob: impl AsRef<Blob>,
    iterator: impl Iterator<Item = usize>,
) -> anyhow::Result<Vec<U256>> {
    let mut field_elements = vec![];
    for index in iterator.map(|i| 32 * i) {
        let bytes: [u8; 32] = blob.as_ref().0[index..index + 32].try_into()?;
        let fe = U256::from_be_bytes(bytes);
        if !fe.cmp(&BLS_MODULUS).is_lt() {
            bail!("Invalid field element at index {index}.");
        }
        field_elements.push(fe);
    }
    Ok(field_elements)
}

/// Reduces a 32-byte hash modulo the BLS modulus, mirroring the on-chain `KailuaKZGLib.hashToFe`.
pub fn hash_to_fe(hash: B256) -> U256 {
    U256::from_be_bytes(hash.0) % BLS_MODULUS
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod tests {
    use super::*;
    use alloy_eips::eip4844::{BYTES_PER_BLOB, BYTES_PER_COMMITMENT, BYTES_PER_PROOF};
    use alloy_primitives::keccak256;
    use alloy_rpc_types_beacon::sidecar::BlobData;
    use rayon::prelude::*;
    use rkyv::rancor::Error;

    pub fn gen_blobs(count: usize) -> Vec<Blob> {
        (0..count)
            .map(|i| {
                (0..FIELD_ELEMENTS_PER_BLOB)
                    .map(|j| {
                        hash_to_fe(keccak256(format!("gen_blobs {i} {j}"))).to_be_bytes::<32>()
                    })
                    .collect::<Vec<_>>()
                    .concat()
                    .as_slice()
                    .try_into()
                    .unwrap()
            })
            .collect()
    }

    #[test]
    fn test_hash_to_fe() {
        for i in 0..1024 {
            let hash = keccak256(format!("test_hash_to_fe hash {i}"));
            let fe = hash_to_fe(hash);
            assert_eq!(fe, hash_to_fe(fe.to_be_bytes().into()));
        }
    }

    #[test]
    fn test_field_elements() {
        let blobs = gen_blobs(64);
        for (i, blob) in blobs.into_iter().enumerate() {
            let blob_data = BlobData {
                index: 0,
                blob: Box::new(blob),
                kzg_commitment: Default::default(),
                kzg_proof: Default::default(),
                signed_block_header: Default::default(),
                kzg_commitment_inclusion_proof: vec![],
            };
            let blocks = 64 * i;
            let recovered_bytes = [
                intermediate_outputs(&blob_data.blob, blocks).unwrap(),
                trail_data(&blob_data.blob, blocks).unwrap(),
            ]
            .concat()
            .into_iter()
            .map(|e| e.to_be_bytes::<32>())
            .collect::<Vec<_>>()
            .concat();
            assert_eq!(blob.0.as_slice(), recovered_bytes.as_slice());
        }
    }

    #[test]
    fn test_field_element_fail() {
        let mut blob = gen_blobs(1).pop().unwrap();
        blob.0[0] = 0xff;
        let blob = Box::new(blob);
        intermediate_outputs(&blob, 1).unwrap_err();
    }

    #[test]
    fn test_preloaded_blob_provider_tampering() {
        let witness_data = BlobWitnessData::from(gen_blobs(1));
        // Fail if any bit is wrong
        for i in 0..witness_data.blobs.len() {
            // Tamper with blob data
            (0..BYTES_PER_BLOB).into_par_iter().for_each(|j| {
                let mut tampered_witness_data = witness_data.clone();
                tampered_witness_data.blobs[i].0[j] ^= 1;

                assert_ne!(witness_data.blobs[i], tampered_witness_data.blobs[i]);
                let result =
                    std::panic::catch_unwind(|| PreloadedBlobProvider::from(tampered_witness_data));
                assert!(result.is_err());
            });
            // Tamper with commitment
            (0..BYTES_PER_COMMITMENT).into_par_iter().for_each(|j| {
                (0..8usize).into_par_iter().for_each(|k| {
                    let mut tampered_witness_data = witness_data.clone();
                    tampered_witness_data.commitments[i][j] ^= 1 << k;

                    assert_ne!(
                        witness_data.commitments[i],
                        tampered_witness_data.commitments[i]
                    );
                    let result = std::panic::catch_unwind(|| {
                        PreloadedBlobProvider::from(tampered_witness_data)
                    });
                    assert!(result.is_err());
                });
            });
            // Tamper with proof
            (0..BYTES_PER_PROOF).into_par_iter().for_each(|j| {
                (0..8usize).into_par_iter().for_each(|k| {
                    let mut tampered_witness_data = witness_data.clone();
                    tampered_witness_data.proofs[i][j] ^= 1 << k;

                    assert_ne!(witness_data.proofs[i], tampered_witness_data.proofs[i]);
                    let result = std::panic::catch_unwind(|| {
                        PreloadedBlobProvider::from(tampered_witness_data)
                    });
                    assert!(result.is_err());
                });
            });
        }
        // Succeed on genuine data
        let _ = PreloadedBlobProvider::from(witness_data);
    }

    #[tokio::test]
    async fn test_blob_provider() {
        let blobs = gen_blobs(32);
        let blob_witness_data = BlobWitnessData::from(blobs.clone());
        // serde
        let blob_witness_data = rkyv::from_bytes::<BlobWitnessData, Error>(
            rkyv::to_bytes::<Error>(&blob_witness_data)
                .unwrap()
                .as_ref(),
        )
        .unwrap();
        let hashes = blob_witness_data
            .commitments
            .iter()
            .map(|c| kzg_to_versioned_hash(c.as_slice()))
            .collect::<Vec<_>>();
        let mut blob_provider = PreloadedBlobProvider::from(blob_witness_data);
        let retrieved = blob_provider
            .get_and_validate_blobs(&Default::default(), &hashes)
            .await
            .unwrap()
            .into_iter()
            .map(|b| *b)
            .collect::<Vec<_>>();

        assert_eq!(blobs, retrieved);
    }

    #[tokio::test]
    async fn test_blob_provider_bad_query() {
        let blobs = gen_blobs(32);
        let blob_witness_data = BlobWitnessData::from(blobs.clone());
        // exhaust the provider and find nothing
        let hashes = blob_witness_data
            .commitments
            .iter()
            .map(|c| !kzg_to_versioned_hash(c.as_slice())) // invert the expected hash
            .collect::<Vec<_>>();
        let mut blob_provider = PreloadedBlobProvider::from(blob_witness_data);
        let retrieved = blob_provider
            .get_and_validate_blobs(&Default::default(), &hashes)
            .await;

        assert!(retrieved.is_err());
        assert_eq!(blob_provider.entries.len(), 31);
    }
}

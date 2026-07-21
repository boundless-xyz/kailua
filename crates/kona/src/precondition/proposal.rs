// Copyright 2024, 2025 Boundless Foundation, Inc.
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

use crate::blobs::BlobFetchRequest;
use crate::blobs::hash_to_fe;
use alloy_eips::eip4844::{Blob, FIELD_ELEMENTS_PER_BLOB};
use alloy_primitives::B256;
use anyhow::Context;
use anyhow::bail;
use kona_derive::BlobProvider;
use kona_preimage::{CommsClient, PreimageKey, PreimageKeyType};
use kona_proof::errors::OracleProviderError;
use risc0_zkvm::sha::Impl as SHA2;
use risc0_zkvm::sha::Sha256;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt::Debug;
use std::iter::once;
use std::sync::Arc;

/// The data required to validate the intermediate output roots published in a proposal against
/// the outputs computed by a proof.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProposalPrecondition {
    /// Block height of the proposal's starting (agreed) L2 output root.
    pub proposal_l2_head_number: u64,
    /// Number of output roots the proposal commits to.
    pub proposal_output_count: u64,
    /// Number of L2 blocks covered by each output root.
    pub output_block_span: u64,
    /// One blob fetch request per blob published with the proposal.
    pub blob_hashes: Vec<BlobFetchRequest>,
}

impl ProposalPrecondition {
    /// Serializes the precondition with `pot`, panicking on failure.
    pub fn to_vec(&self) -> Vec<u8> {
        pot::to_vec(self).unwrap()
    }

    /// The SHA-256 hash of the serialized precondition, used to reference it in oracle storage.
    /// It does not authenticate the precondition, unlike [Self::precondition_hash].
    pub fn hash(&self) -> B256 {
        let digest = *SHA2::hash_bytes(&self.to_vec());
        B256::from_slice(digest.as_bytes())
    }

    /// The blob fetch requests, one per blob published with the proposal.
    pub fn blob_fetch_requests(&self) -> &[BlobFetchRequest] {
        self.blob_hashes.as_slice()
    }

    /// The SHA-256 hash of the concatenated versioned hashes of the proposal's blobs.
    pub fn blobs_hash(&self) -> B256 {
        blobs_hash(self.blob_fetch_requests().iter().map(|b| &b.blob_hash.hash))
    }

    /// Computes the proposal precondition hash committed to in the proof journal.
    pub fn precondition_hash(&self) -> B256 {
        proposal_precondition_hash(
            &self.proposal_l2_head_number,
            &self.proposal_output_count,
            &self.output_block_span,
            blobs_hash(self.blob_hashes.iter().map(|b| &b.blob_hash.hash)),
        )
    }
}

/// Computes the SHA-256 hash tying a proof to a specific proposal: its starting L2 block
/// number, output count, block span per output, and the hash of its published blobs.
pub fn proposal_precondition_hash(
    proposal_l2_head_number: &u64,
    proposal_output_count: &u64,
    output_block_span: &u64,
    blobs_hash: B256,
) -> B256 {
    let phn_bytes = proposal_l2_head_number.to_be_bytes();
    let poc_bytes = proposal_output_count.to_be_bytes();
    let obs_bytes = output_block_span.to_be_bytes();
    let all_bytes = once(phn_bytes.as_slice())
        .chain(once(poc_bytes.as_slice()))
        .chain(once(obs_bytes.as_slice()))
        .chain(once(blobs_hash.as_slice()))
        .collect::<Vec<_>>()
        .concat();
    let digest = *SHA2::hash_bytes(&all_bytes);
    B256::from_slice(digest.as_bytes())
}

/// Computes the SHA-256 hash of the concatenated blob hashes.
pub fn blobs_hash<'a>(blob_hashes: impl Iterator<Item = &'a B256>) -> B256 {
    let blobs_hash_bytes = blob_hashes
        .map(|h| h.as_slice())
        .collect::<Vec<_>>()
        .concat();
    let digest = *SHA2::hash_bytes(&blobs_hash_bytes);
    B256::from_slice(digest.as_bytes())
}

/// Loads the [ProposalPrecondition] referenced by `proposal_data_hash` from the oracle, along
/// with every blob it commits to, each validated by the provider against its versioned hash.
/// Returns `None` when the hash is zero, denoting the absence of a proposal precondition.
pub async fn load_proposal_data<
    O: CommsClient + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
>(
    proposal_data_hash: B256,
    oracle: Arc<O>,
    beacon: &mut B,
) -> anyhow::Result<Option<(ProposalPrecondition, Vec<Blob>)>>
where
    <B as BlobProvider>::Error: Debug,
{
    if proposal_data_hash.is_zero() {
        return Ok(None);
    }
    // Read the blob references to fetch
    let precondition_validation_data: ProposalPrecondition = pot::from_slice(
        &oracle
            .get(PreimageKey::new(
                *proposal_data_hash,
                PreimageKeyType::Sha256,
            ))
            .await
            .map_err(OracleProviderError::Preimage)?,
    )
    .context("Pot::from_slice")?;
    let mut blobs = Vec::new();
    // Read the blob data corresponding to the supplied blob hashes
    for request in precondition_validation_data.blob_fetch_requests() {
        blobs.push(
            *beacon
                .get_and_validate_blobs(
                    &request.block_ref,
                    std::slice::from_ref(&request.blob_hash.hash),
                )
                .await
                .unwrap()[0],
        );
    }

    Ok(Some((precondition_validation_data, blobs)))
}

/// Verifies that the output roots computed by the proof match the intermediate outputs the
/// proposal published in its blobs, returning the proposal precondition hash on success.
///
/// The proof's block range must fall within the proposal's range. Every computed output root at
/// an `output_block_span` boundary must equal, as a field element, the blob element at its
/// offset, and the blob data trailing the final output must be all zeros. These mirror the
/// output-fault and trail-fault conditions the `KailuaTournament` contract accepts, so no
/// proposal can be proven both valid and faulty.
///
/// Assumes the supplied blobs were already validated against the precondition's blob hashes.
pub fn validate_proposal_precondition(
    precondition_validation_data: ProposalPrecondition,
    blobs: Vec<Blob>,
    proof_l2_head_number: u64,
    output_roots: &[B256],
) -> anyhow::Result<B256> {
    let precondition_hash = precondition_validation_data.precondition_hash();
    let ProposalPrecondition {
        proposal_l2_head_number,
        proposal_output_count,
        output_block_span,
        .. // `blob`/`blob_hashse` correspondence assumed to have been already validated
    } = precondition_validation_data;
    let proposal_root_claim_block_number =
        proposal_l2_head_number + proposal_output_count * output_block_span;
    // Ensure local and global block ranges match
    if proof_l2_head_number < proposal_l2_head_number {
        bail!(
            "Validity precondition proposal starting block #{proposal_l2_head_number} > proof agreed l2 head #{proof_l2_head_number}"
        )
    } else if proposal_root_claim_block_number < proof_l2_head_number {
        bail!(
            "Validity precondition proposal ending block #{proposal_l2_head_number} < proof agreed l2 head #{proof_l2_head_number}"
        )
    } else if output_roots.is_empty() {
        // abort early if no validation is to take place
        return Ok(precondition_hash);
    }
    // Calculate blob index pointer
    for (i, output_hash) in output_roots.iter().enumerate() {
        let output_block_number = proof_l2_head_number + i as u64 + 1;
        if output_block_number > proposal_root_claim_block_number {
            // We should not derive outputs beyond the proposal root claim
            bail!(
                "Output block #{output_block_number} > max block #{proposal_root_claim_block_number}."
            );
        }
        let offset = output_block_number - proposal_l2_head_number;
        if !offset.is_multiple_of(output_block_span) {
            // We only check equivalence every output_block_span blocks
            continue;
        }
        let intermediate_output_offset = (offset / output_block_span) - 1;
        let blob_index = (intermediate_output_offset / FIELD_ELEMENTS_PER_BLOB) as usize;
        let fe_position = (intermediate_output_offset % FIELD_ELEMENTS_PER_BLOB) as usize;
        let blob_fe_index = 32 * fe_position;
        // Verify fe equivalence to computed outputs for all but last output
        match intermediate_output_offset.cmp(&(proposal_output_count - 1)) {
            Ordering::Less => {
                // verify equivalence to blob
                let blob_fe_slice = &blobs[blob_index][blob_fe_index..blob_fe_index + 32];
                let output_fe = hash_to_fe(*output_hash);
                let output_fe_bytes = output_fe.to_be_bytes::<32>();
                if blob_fe_slice != output_fe_bytes.as_slice() {
                    bail!(
                        "Bad fe #{} in blob {} for block #{}: Expected {} found {} ",
                        fe_position,
                        blob_index,
                        output_block_number,
                        B256::try_from(output_fe_bytes.as_slice())?,
                        B256::try_from(blob_fe_slice)?
                    );
                }
            }
            Ordering::Equal => {
                if proposal_output_count > 1 {
                    // verify zeroed trail data
                    if blob_index != blobs.len() - 1 {
                        bail!(
                            "Expected trail data to begin at blob {blob_index}/{}",
                            blobs.len()
                        );
                    } else if blobs[blob_index][blob_fe_index..].iter().any(|b| b != &0u8) {
                        bail!(
                            "Found non-zero trail data in blob {blob_index} after {blob_fe_index}"
                        );
                    }
                }
            }
            Ordering::Greater => {
                // (output_block_number <= max_block_number) implies:
                // (output_offset <= proposal_output_count)
                unreachable!(
                    "Output offset {intermediate_output_offset} > output count {proposal_output_count}."
                );
            }
        }
    }
    // Return the precondition hash
    Ok(precondition_hash)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::blobs::tests::gen_blobs;
    use crate::blobs::{BlobWitnessData, PreloadedBlobProvider, intermediate_outputs};
    use crate::oracle::WitnessOracle;
    use crate::oracle::vec::tests::prepare_vec_oracle;
    use crate::precondition::proposal::{
        ProposalPrecondition, load_proposal_data, proposal_precondition_hash,
        validate_proposal_precondition,
    };
    use alloy_eips::eip4844::{BYTES_PER_BLOB, IndexedBlobHash, kzg_to_versioned_hash};
    use kona_proof::block_on;
    use rayon::prelude::{IntoParallelIterator, ParallelIterator};

    pub fn gen_blobs_requests(blobs: Vec<Blob>) -> Vec<BlobFetchRequest> {
        let blobs_witness = BlobWitnessData::from(blobs);
        let blobs_hashes = blobs_witness
            .commitments
            .iter()
            .map(|c| kzg_to_versioned_hash(c.as_slice()))
            .collect::<Vec<_>>();
        blobs_hashes
            .iter()
            .copied()
            .map(|hash| BlobFetchRequest {
                block_ref: Default::default(),
                blob_hash: IndexedBlobHash { index: 0, hash },
            })
            .collect::<Vec<_>>()
    }

    #[tokio::test]
    async fn test_load_precondition_data() {
        let max_blobs = 6;
        (1..=max_blobs).into_par_iter().for_each(|n| {
            // println!("Testing with {n} blobs");
            let blobs = gen_blobs(n);
            // create remaining dummy data
            let blobs_witness = BlobWitnessData::from(blobs);
            let blobs_hashes = blobs_witness
                .commitments
                .iter()
                .map(|c| kzg_to_versioned_hash(c.as_slice()))
                .collect::<Vec<_>>();
            let beacon = PreloadedBlobProvider::from(blobs_witness);
            let blobs_fetch_requests = blobs_hashes
                .iter()
                .copied()
                .map(|hash| BlobFetchRequest {
                    block_ref: Default::default(),
                    blob_hash: IndexedBlobHash { index: 0, hash },
                })
                .collect::<Vec<_>>();
            // The number of outputs published is the root claim + non-zero blob elements
            let proposal_output_count =
                1 + (n as u64) * FIELD_ELEMENTS_PER_BLOB - FIELD_ELEMENTS_PER_BLOB / 2;
            // Test over different configurations
            for proposal_l2_head_number in [1, 2, 5, 7, proposal_output_count] {
                // println!("Testing with {proposal_l2_head_number} L2 head");
                for output_block_span in [1, 2, 7, 11, 13] {
                    // println!("Testing with {output_block_span} output block span");
                    let precondition_validation_data = ProposalPrecondition {
                        proposal_l2_head_number,
                        proposal_output_count,
                        output_block_span,
                        blob_hashes: blobs_fetch_requests.clone(),
                    };
                    // test data loading
                    let precondition_data_hash = precondition_validation_data.hash();
                    let mut oracle = prepare_vec_oracle(0, 0).0;
                    oracle.insert_preimage(
                        PreimageKey::new(precondition_data_hash.0, PreimageKeyType::Sha256),
                        precondition_validation_data.to_vec(),
                    );
                    let oracle = Arc::new(oracle);
                    // load nothing when hash is zero
                    assert!(
                        block_on(load_proposal_data(
                            B256::ZERO,
                            oracle.clone(),
                            &mut beacon.clone(),
                        ))
                        .unwrap()
                        .is_none()
                    );
                    // successfully load with proper hash
                    let reloaded = block_on(load_proposal_data(
                        precondition_data_hash,
                        oracle.clone(),
                        &mut beacon.clone(),
                    ))
                    .unwrap()
                    .unwrap()
                    .0;
                    assert_eq!(reloaded, precondition_validation_data);
                }
            }
        });
    }

    #[test]
    fn test_validate_precondition_bad_start() {
        assert!(
            validate_proposal_precondition(
                ProposalPrecondition {
                    proposal_l2_head_number: 100,
                    proposal_output_count: 100,
                    output_block_span: 1,
                    blob_hashes: vec![],
                },
                vec![],
                1,
                &[]
            )
            .is_err_and(|e| e
                .to_string()
                .contains("proposal starting block #100 > proof agreed l2 head #1"))
        );
    }

    #[test]
    fn test_validate_precondition_tamper() {
        let blobs = gen_blobs(2);
        let blobs_fetch_requests = gen_blobs_requests(blobs.clone());
        // fail to validate trail with too many blobs
        let output_roots = intermediate_outputs(Box::new(blobs[0]), 1024)
            .unwrap()
            .into_iter()
            .map(|fe| B256::from(fe.to_be_bytes::<32>()))
            .collect::<Vec<_>>();
        let result = validate_proposal_precondition(
            ProposalPrecondition {
                proposal_l2_head_number: 1,
                proposal_output_count: 1024,
                output_block_span: 1,
                blob_hashes: blobs_fetch_requests.clone(),
            },
            blobs.clone(),
            1,
            &output_roots,
        );
        assert!(result.is_err_and(|e| {
            e.to_string()
                .contains("Expected trail data to begin at blob 0/2")
        }));
        // fail to validate non-zero trail data after 1023 * 32 = 32768 bytes
        let result = validate_proposal_precondition(
            ProposalPrecondition {
                proposal_l2_head_number: 1,
                proposal_output_count: 1024,
                output_block_span: 1,
                blob_hashes: blobs_fetch_requests[..1].to_vec(),
            },
            blobs[..1].to_vec(),
            1,
            &output_roots,
        );
        assert!(result.is_err_and(|e| {
            e.to_string()
                .contains("Found non-zero trail data in blob 0 after 32736")
        }));
        // fail to validate extra output roots
        let mut blobs = blobs[..1].to_vec();
        let blobs_fetch_requests = gen_blobs_requests(blobs.clone());
        for blob_byte in blobs[0].iter_mut().skip(500 * 32).take(32) {
            *blob_byte = !*blob_byte;
        }
        let result = validate_proposal_precondition(
            ProposalPrecondition {
                proposal_l2_head_number: 1,
                proposal_output_count: 1024,
                output_block_span: 1,
                blob_hashes: blobs_fetch_requests,
            },
            blobs,
            1,
            &output_roots,
        );
        assert!(result.is_err_and(|e| {
            e.to_string()
                .contains("Bad fe #500 in blob 0 for block #502")
        }));
    }

    #[tokio::test]
    async fn test_validate_precondition() {
        let m = BYTES_PER_BLOB / 2;
        // test for various blob counts
        let max_blobs = 6;
        (1..=max_blobs).into_par_iter().for_each(|n| {
            // println!("Testing with {n} blobs");
            let mut blobs = gen_blobs(n);
            // Zero out the last half of the last blob
            for blob_byte in blobs[n - 1].iter_mut().skip(m) {
                *blob_byte = 0;
            }
            // create remaining dummy data
            let blobs_fetch_requests = gen_blobs_requests(blobs.clone());
            // The number of outputs published is the root claim + non-zero blob elements
            let proposal_output_count =
                1 + (n as u64) * FIELD_ELEMENTS_PER_BLOB - FIELD_ELEMENTS_PER_BLOB / 2;
            // Test over different configurations
            for proposal_l2_head_number in [1, 2, 5, 7, proposal_output_count] {
                // println!("Testing with {proposal_l2_head_number} L2 head");
                for output_block_span in [1, 2, 7, 11, 13] {
                    // println!("Testing with {output_block_span} output block span");
                    let precondition_validation_data = ProposalPrecondition {
                        proposal_l2_head_number,
                        proposal_output_count,
                        output_block_span,
                        blob_hashes: blobs_fetch_requests.clone(),
                    };
                    // check requests referencing
                    assert_eq!(
                        precondition_validation_data.blob_fetch_requests(),
                        blobs_fetch_requests.as_slice()
                    );
                    // test serde
                    {
                        let recoded =
                            pot::from_slice(precondition_validation_data.to_vec().as_slice())
                                .unwrap();
                        assert_eq!(precondition_validation_data, recoded);
                    }
                    // check hashing
                    let precondition_hash = proposal_precondition_hash(
                        &proposal_l2_head_number,
                        &proposal_output_count,
                        &output_block_span,
                        precondition_validation_data.blobs_hash(),
                    );
                    assert_eq!(
                        precondition_hash,
                        precondition_validation_data.precondition_hash()
                    );
                    // test over different subsequences
                    let max_offset = (n as u64) * FIELD_ELEMENTS_PER_BLOB;
                    let starting_points = (0..max_blobs as u64)
                        .flat_map(|i| {
                            vec![
                                i * FIELD_ELEMENTS_PER_BLOB,
                                i * FIELD_ELEMENTS_PER_BLOB + FIELD_ELEMENTS_PER_BLOB / 2,
                            ]
                        })
                        .collect::<Vec<_>>();
                    for starting_offset in starting_points {
                        let ending_points = (0..n as u64)
                            .flat_map(|i| {
                                vec![
                                    i * FIELD_ELEMENTS_PER_BLOB,
                                    i * FIELD_ELEMENTS_PER_BLOB + FIELD_ELEMENTS_PER_BLOB / 2,
                                ]
                            })
                            .map(|p| p + starting_offset)
                            .collect::<Vec<_>>();
                        for ending_offset in ending_points {
                            let output_roots: Vec<B256> = (starting_offset..ending_offset)
                                .filter(|i| *i < max_offset)
                                .flat_map(|i| {
                                    let bi = (i / FIELD_ELEMENTS_PER_BLOB) as usize;
                                    let fi = (i % FIELD_ELEMENTS_PER_BLOB) as usize;
                                    // replicate the target output as needed
                                    vec![
                                        blobs[bi][fi * 32..(fi + 1) * 32].try_into().unwrap();
                                        output_block_span as usize
                                    ]
                                })
                                .collect();

                            let proof_l2_head_number =
                                proposal_l2_head_number + starting_offset * output_block_span;
                            let result = validate_proposal_precondition(
                                precondition_validation_data.clone(),
                                blobs.clone(),
                                proof_l2_head_number,
                                &output_roots,
                            );
                            if starting_offset < max_offset && ending_offset < max_offset {
                                // println!("Testing starting offset {starting_offset} ending offset {ending_offset}");
                                // check correct validation
                                assert_eq!(precondition_hash, result.unwrap());
                            } else if starting_offset < max_offset {
                                // fail the attempt to continue validating beyond max block
                                assert!(
                                    result.is_err_and(|e| e.to_string().contains("> max block"))
                                );
                            } else {
                                // fail the attempt to start validating beyond max block
                                assert!(result.is_err_and(|e| {
                                    e.to_string().contains("< proof agreed l2 head")
                                }));
                            }
                        }
                    }
                }
            }
        });
    }
}

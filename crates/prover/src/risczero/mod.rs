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

use crate::args::ProvingArgs;
use crate::profiling::{Profile, ProfiledReceipt};
use crate::proof::proof_file_name;
use crate::risczero::boundless::BoundlessArgs;
use crate::{ProvingError, current_time, proof};
use anyhow::Context;
use risc0_zkvm::{Journal, Receipt};
use std::convert::identity;
use std::path::{Path, PathBuf};
use tracing::{error, info};

/// Proving via the Bonsai remote proving service.
pub mod bonsai;
/// Proving via the Boundless proving market.
pub mod boundless;
/// Proving via the local zkVM prover.
pub mod zkvm;

/// Exhaustive stand-in for the non-exhaustive [risc0_zkvm::SessionStats].
#[derive(Debug, Clone)]
pub struct KailuaSessionStats {
    /// Number of proven segments.
    pub segments: usize,
    /// Total cycles proven.
    pub total_cycles: u64,
    /// Cycles spent executing user code.
    pub user_cycles: u64,
    /// Cycles spent paging memory in and out.
    pub paging_cycles: u64,
    /// Cycles reserved by the prover.
    pub reserved_cycles: u64,
}

/// Exhaustive stand-in for the non-exhaustive [risc0_zkvm::ProveInfo].
#[derive(Debug)]
pub struct KailuaProveInfo {
    /// The computed receipt.
    pub receipt: Receipt,
    /// The proving session statistics.
    pub stats: KailuaSessionStats,
}

/// Computes and saves a proof of the journal through the appropriate backend.
///
/// Uses the Boundless market when configured (outside dev mode), otherwise Bonsai when its API
/// env vars are set, and the local zkVM prover as the fallback. The resulting receipt is
/// checked against the expected journal and written to its proof file with its profile.
#[allow(clippy::too_many_arguments)]
#[allow(deprecated)]
pub async fn seek_proof(
    proving: &ProvingArgs,
    boundless: BoundlessArgs,
    journal: Journal,
    witness_slices: Vec<Vec<u32>>,
    witness_frames: Vec<Vec<u8>>,
    stitched_proofs: Vec<ProfiledReceipt>,
    prove_snark: bool,
    mut profile: Profile,
    data_dir: Option<&PathBuf>,
) -> Result<(), ProvingError> {
    // Check proof cache
    let file_name = proof_file_name(proving.image_id(), journal.clone());
    if Path::new(&file_name).try_exists().is_ok_and(identity) {
        info!("Proving skipped. Proof file {file_name} already exists.");
    }

    // separate sub-proof data
    let stitched_receipts = stitched_proofs
        .iter()
        .map(|r| r.0.clone())
        .collect::<Vec<_>>();
    // accrue input data
    profile = profile
        .with_start_time(current_time())
        .with_witness_frames(&witness_frames)
        .with_proofs(proving.image().0, &stitched_proofs);

    // compute the zkvm proof
    let mut proof = match (boundless.market, boundless.storage) {
        (Some(marked_provider_config), Some(storage_provider_config))
            if !risc0_zkvm::is_dev_mode() =>
        {
            boundless::run_boundless_client(
                marked_provider_config,
                storage_provider_config,
                boundless.r2_domain,
                proving.image(),
                journal.clone(),
                witness_slices,
                witness_frames,
                stitched_receipts,
                proving,
                profile,
                data_dir,
            )
            .await?
        }
        _ => {
            if bonsai::should_use_bonsai() {
                bonsai::run_bonsai_client(
                    proving.image(),
                    witness_slices,
                    witness_frames,
                    stitched_receipts,
                    prove_snark,
                    proving,
                    profile,
                )
                .await?
            } else {
                zkvm::run_zkvm_client(
                    proving.image(),
                    witness_slices,
                    witness_frames,
                    stitched_receipts,
                    prove_snark,
                    proving,
                    profile,
                )
                .await?
            }
        }
    };

    // accumulate sub-proof data
    proof.1 = proof.1.with_finish_time(current_time());

    // Save proof file to disk
    if journal != proof.0.journal {
        error!(
            "Expected journal {} but found {}",
            hex::encode(&journal),
            hex::encode(&proof.0.journal)
        );
    }
    let file_name = proof_file_name(proving.image_id(), proof.0.journal.clone());
    proof::save_to_bincoded_file(&proof, None, &file_name)
        .await
        .context("save_to_bincoded_file")
        .map_err(ProvingError::OtherError)?;
    info!("Saved proof to file {file_name}");

    Ok(())
}

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

use crate::blobs::PreloadedBlobProvider;
use crate::client::log;
use crate::client::stitching::StitchingClient;
use crate::journal::ProofJournal;
use crate::oracle::WitnessOracle;
use crate::witness::Witness;
use std::sync::Arc;

/// Runs the guest program over a self-contained [Witness], returning the resulting [ProofJournal].
///
/// The oracle witness's preimages are validated up front, so every response served during the run
/// is authenticated data; validation failure panics. The provided stream witness is discarded in
/// favor of an empty oracle, and blobs are served through the KZG-authenticated
/// [PreloadedBlobProvider]. Preimages left unconsumed after the run are logged, not treated as
/// errors.
pub fn run_stateless_client<O: WitnessOracle, S: StitchingClient<O, PreloadedBlobProvider>>(
    witness: Witness<O>,
    stitching_client: S,
) -> ProofJournal {
    #[cfg(all(feature = "experimental", target_os = "zkvm", target_vendor = "risc0"))]
    crate::r0vm_crypto::install_r0vm_crypto();

    log(&format!(
        "ORACLE: {} PREIMAGES",
        witness.oracle_witness.preimage_count()
    ));
    witness
        .oracle_witness
        .validate_preimages()
        .expect("Failed to validate preimages");
    let oracle = Arc::new(witness.oracle_witness);
    // ignore the provided stream witness if any
    let stream = Arc::new(O::default());
    log(&format!(
        "BEACON: {} BLOBS",
        witness.blobs_witness.blobs.len()
    ));
    let beacon = PreloadedBlobProvider::from(witness.blobs_witness);

    let (_, proof_journal, _) = stitching_client.run_stitching_client(
        witness.precondition_validation_data_hash,
        oracle.clone(),
        stream,
        beacon,
        witness.fpvm_image_id,
        witness.payout_recipient_address,
        witness.stitched_executions,
        witness.derivation_cache,
        witness.trace_derivation,
        witness.stitched_preconditions,
        witness.stitched_boot_info,
        #[cfg(feature = "experimental")]
        witness.pe_witness,
        #[cfg(feature = "experimental")]
        witness.partial_executions,
    );

    if oracle.preimage_count() > 0 {
        log(&format!("EXTRA PREIMAGES: {}", oracle.preimage_count()));
    }

    proof_journal
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod tests {
    use super::*;
    use crate::client::core::EthereumDataSourceProvider;
    #[cfg(feature = "experimental")]
    use crate::client::core::tests::test_derivation_with_partials;
    use crate::client::core::tests::{op_sepolia_16491249_16491349, test_derivation};
    use crate::client::stitching::KonaStitchingClient;
    use crate::client::tests::TestOracle;
    use alloy_primitives::B256;
    use anyhow::Context;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_stateless_client() -> anyhow::Result<()> {
        let mut boot_info = op_sepolia_16491249_16491349();
        let stitched_executions = test_derivation(boot_info.clone(), None, None, None)
            .await
            .context("test_derivation")?;
        boot_info.l1_head = B256::ZERO;
        let oracle_witness = TestOracle::new(boot_info.clone());
        let stream_witness = oracle_witness.clone();
        let witness = Witness {
            oracle_witness,
            stream_witness,
            blobs_witness: Default::default(),
            payout_recipient_address: Default::default(),
            precondition_validation_data_hash: Default::default(),
            stitched_executions: vec![stitched_executions],
            derivation_cache: None,
            trace_derivation: false,
            stitched_preconditions: vec![],
            stitched_boot_info: vec![],
            fpvm_image_id: Default::default(),
            #[cfg(feature = "experimental")]
            pe_witness: None,
            #[cfg(feature = "experimental")]
            partial_executions: Vec::new(),
        };

        run_stateless_client(witness, KonaStitchingClient(EthereumDataSourceProvider));

        Ok(())
    }

    #[cfg(feature = "experimental")]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_stateless_client_with_partials() -> anyhow::Result<()> {
        let mut boot_info = op_sepolia_16491249_16491349();

        // capture
        let (stitched_executions, partial_executions) = test_derivation_with_partials(
            boot_info.clone(),
            None,
            None,
            Some(Default::default()),
            Vec::new(),
        )
        .await
        .context("capture pass")?;
        assert!(!stitched_executions.is_empty());
        assert_eq!(stitched_executions.len(), partial_executions.len());

        // replay
        boot_info.l1_head = B256::ZERO;
        let oracle_witness = TestOracle::new(boot_info.clone());
        let stream_witness = oracle_witness.clone();
        let witness = Witness {
            oracle_witness,
            stream_witness,
            blobs_witness: Default::default(),
            payout_recipient_address: Default::default(),
            precondition_validation_data_hash: Default::default(),
            stitched_executions: vec![stitched_executions],
            derivation_cache: None,
            trace_derivation: false,
            stitched_preconditions: vec![],
            stitched_boot_info: vec![],
            fpvm_image_id: Default::default(),
            pe_witness: None,
            partial_executions,
        };

        run_stateless_client(witness, KonaStitchingClient(EthereumDataSourceProvider));

        Ok(())
    }
}

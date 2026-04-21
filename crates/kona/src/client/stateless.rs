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

use crate::blobs::PreloadedBlobProvider;
use crate::client::log;
use crate::client::stitching::StitchingClient;
use crate::journal::ProofJournal;
use crate::oracle::WitnessOracle;
use crate::witness::Witness;
use std::sync::Arc;

/// Executes a stateless client workflow by validating witness data, and running the stitching
/// client to produce a unified proof journal.
///
/// # Arguments
/// * `witness`: A `Witness<O>` object that contains all the input data required to execute the stateless client.
///
/// # Returns
/// * `ProofJournal`: The resulting proof journal from running the stitching client.
///
/// # Function Details
/// 1. Logs information about the number of "preimages" in the oracle witness.
/// 2. Validates the oracle witness's preimages through `validate_preimages`. If validation fails, the program will panic with an error message.
/// 3. Wraps the constructed oracle witness in an `Arc` for shared ownership and thread safety.
/// 4. Initializes a default stream witness of type `O` (provided by the generic parameter) and wraps it in an `Arc`.
/// 5. Logs information about the number of blobs in the blob witness.
/// 6. Constructs a `PreloadedBlobProvider` instance from the blob witness to manage the blobs.
/// 7. Executes the stitching client via `run_stitching_client`, which combines witness data, preconditions, headers,
///    and execution details. The result is a `ProofJournal` representing the proof output.
/// 8. Checks if any additional preimages have been discovered beyond what was initially provided, logging a warning if so.
///
/// # Panics
/// This function will panic if:
/// * The `validate_preimages` function call on the oracle witness fails, indicating invalid witness data.
///
/// # Logging
/// * Logs the count of preimages provided via the `oracle_witness`.
/// * Logs the count of blobs contained in the `blobs_witness`.
/// * Logs a warning if any extra preimages are found during execution.
pub fn run_stateless_client<O: WitnessOracle, S: StitchingClient<O, PreloadedBlobProvider>>(
    witness: Witness<O>,
    stitching_client: S,
) -> ProofJournal {
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
        witness.chunk_witness,
        witness.chunks,
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
    use crate::client::core::tests::test_derivation;
    use crate::client::core::EthereumDataSourceProvider;
    use crate::client::stitching::KonaStitchingClient;
    use crate::client::tests::TestOracle;
    use alloy_primitives::{b256, B256};
    use anyhow::Context;
    use kona_proof::BootInfo;

    #[test]
    fn test_stateless_client() -> anyhow::Result<()> {
        let mut boot_info = BootInfo {
            l1_head: b256!("0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"),
            agreed_l2_output_root: b256!(
                "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
            ),
            claimed_l2_output_root: b256!(
                "0x6984e5ae4d025562c8a571949b985692d80e364ddab46d5c8af5b36a20f611d1"
            ),
            claimed_l2_block_number: 16491349,
            chain_id: 11155420,
            rollup_config: Default::default(),
            l1_config: Default::default(),
        };
        let stitched_executions = test_derivation(boot_info.clone(), None, None, None)
            .context("test_derivation")?
            .into_iter()
            .map(|e| e.as_ref().clone())
            .collect::<Vec<_>>();
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
            chunk_witness: None,
            chunks: Vec::new(),
        };

        run_stateless_client(witness, KonaStitchingClient(EthereumDataSourceProvider));

        Ok(())
    }

    /// End-to-end round-trip through `run_stateless_client` with a non-empty
    /// `Witness.chunks`. Codex round-4 [high] finding regression test — proves the
    /// transaction-chunk-proving code path is reachable in the standard stateless
    /// replay, not just the `run_core_client` integration tests.
    ///
    /// Follows the existing `test_stateless_client` pattern exactly — captures real
    /// derivation data via `test_derivation_with_chunks_and_traces` (using the
    /// cached `testdata/` `TestOracle` fixture), then sets `l1_head = ZERO` for pass
    /// 2 to route through the EXECUTION-ONLY branch. No blob witness required
    /// because derivation isn't re-run — we replay each Execution from the cache
    /// through `ChunkingEvmFactory` (seeded with the built chunks), and the CHUNK
    /// VERIFY phase in EXECUTION-ONLY authenticates each chunk against its
    /// Execution (same cross-checks as the DERIVATION branch).
    #[tokio::test(flavor = "multi_thread")]
    async fn test_stateless_client_with_chunks() -> anyhow::Result<()> {
        use crate::client::core::tests::{
            build_single_chunk_for_block, test_derivation_with_chunks_and_traces,
            test_fetch_safe_head_context,
        };
        use crate::evm::PartialExecution;
        use std::collections::HashMap;
        use std::sync::{Arc, Mutex};

        let mut boot_info = BootInfo {
            l1_head: b256!("0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"),
            agreed_l2_output_root: b256!(
                "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
            ),
            claimed_l2_output_root: b256!(
                "0x6984e5ae4d025562c8a571949b985692d80e364ddab46d5c8af5b36a20f611d1"
            ),
            claimed_l2_block_number: 16491349,
            chain_id: 11155420,
            rollup_config: Default::default(),
            l1_config: Default::default(),
        };
        let safe_head_number = 16491249u64;

        // ---- Pass 1 (capture): test_derivation_with_chunks_and_traces runs the full
        // derivation through `run_core_client`, captures per-tx `ResultAndState`,
        // returns the Executions.
        let collector: crate::evm::cached::TransactionResultCollector =
            Arc::new(Mutex::new(HashMap::new()));
        let executions = test_derivation_with_chunks_and_traces(
            boot_info.clone(),
            None,
            None,
            Some(Default::default()),
            Vec::new(),
            Some(collector.clone()),
        )
        .context("capture pass")?;
        assert!(!executions.is_empty());

        let (safe_head_header, real_rollup_config) =
            test_fetch_safe_head_context(&boot_info).await?;
        let rollup_config = Arc::new(real_rollup_config);

        // ---- Build Chunks (one single-chunk per block).
        let mut captured = collector.lock().unwrap();
        let chunks: Vec<Vec<PartialExecution>> = executions
            .iter()
            .enumerate()
            .map(|(i, exec)| {
                let block_number = safe_head_number + 1 + i as u64;
                let traces = captured.remove(&block_number).unwrap_or_default();
                let parent_header = if i == 0 {
                    &safe_head_header
                } else {
                    executions[i - 1].artifacts.header.inner()
                };
                let spec_id = rollup_config.spec_id(exec.artifacts.header.inner().timestamp);
                vec![build_single_chunk_for_block(
                    exec,
                    traces,
                    parent_header,
                    spec_id,
                )]
            })
            .collect();
        drop(captured);

        let stitched_executions: Vec<crate::executor::Execution> =
            executions.into_iter().map(|e| e.as_ref().clone()).collect();

        // ---- Pass 2 (stateless replay): l1_head = ZERO routes through the
        // EXECUTION-ONLY branch, which now supports chunks via ChunkingEvmFactory.
        // No blobs_witness required — derivation isn't re-run in this branch.
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
            chunk_witness: None,
            chunks,
        };

        run_stateless_client(witness, KonaStitchingClient(EthereumDataSourceProvider));

        Ok(())
    }
}

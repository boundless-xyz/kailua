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

use crate::boot::StitchedBootInfo;
use crate::client::core::DASourceProvider;
use crate::client::log;
use crate::driver::CachedDriver;
use crate::evm::PartialExecution;
use crate::executor::Execution;
use crate::journal::ProofJournal;
use crate::kona::OracleL1ChainProvider;
use crate::precondition::chunking::{compute_chunk_trace, hash_block_ctx, hash_results};
use crate::precondition::Precondition;
use crate::witness::ChunkWitnessData;
use alloy_primitives::{Address, B256};
use anyhow::Context;
use kona_derive::{BlobProvider, ChainProvider};
use kona_preimage::CommsClient;
use kona_proof::{BootInfo, FlushableCache};
use risc0_zkvm::sha::Digestible;
use std::fmt::Debug;
use std::iter::zip;
use std::sync::Arc;
#[cfg(target_os = "zkvm")]
use {
    alloy_primitives::map::HashSet,
    risc0_zkvm::{serde::Deserializer, sha::Digest, Receipt},
    serde::Deserialize,
};

pub trait StitchingClient<
    O: CommsClient + FlushableCache + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
>
{
    /// Runs the Kailua client to transition the rollup state and combines the result with
    /// other proven contiguous state transitions to yield a single overarching
    /// `ProofJournal` and `Precondition`.
    ///
    /// The returned `BootInfo` instance is what was loaded by the Kona client.
    ///
    /// # Arguments
    ///
    /// * `proposal_data_hash` - The hash of the proposal blob precondition data.
    /// * `oracle` - The client for preloaded communication with the host environment.
    /// * `stream` - The client for streamed communication with the host.
    /// * `beacon` - The blob provider.
    /// * `fpvm_image_id` - A `B256` identifier for the FPVM image to associate with the operations performed.
    /// * `payout_recipient_address` - The Ethereum address (`Address`) where payout rewards are allocated.
    /// * `stitched_executions` - A nested vector of `Execution` objects containing precomputed execution
    ///   proofs to be stitched.
    /// * `derivation_cache`: An initial snapshot to load for the derivation pipeline.
    /// * `derivation_trace`: Whether to capture the final snapshot of the derivation pipeline in the precondition.
    /// * `stitched_preconditions`: A vector of `Precondition` objects for the stitched proofs.
    /// * `stitched_boot_info` - A vector of `StitchedBootInfo` objects describing proofs
    ///   to be stitched together.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn run_stitching_client(
        self,
        proposal_data_hash: B256,
        oracle: Arc<O>,
        stream: Arc<O>,
        beacon: B,
        fpvm_image_id: B256,
        payout_recipient_address: Address,
        stitched_executions: Vec<Vec<Execution>>,
        derivation_cache: Option<CachedDriver>,
        derivation_trace: bool,
        stitched_preconditions: Vec<Precondition>,
        stitched_boot_info: Vec<StitchedBootInfo>,
        chunk_witness: Option<ChunkWitnessData>,
        chunks: Vec<Vec<PartialExecution>>,
    ) -> (BootInfo, ProofJournal, Precondition)
    where
        <B as BlobProvider>::Error: Debug;
}

#[derive(Clone, Debug)]
pub struct KonaStitchingClient<D: Clone + Debug>(pub D);

impl<
        O: CommsClient + FlushableCache + Send + Sync + Debug,
        B: BlobProvider + Send + Sync + Debug + Clone,
        D: DASourceProvider<OracleL1ChainProvider<O>, B> + Clone + Debug,
    > StitchingClient<O, B> for KonaStitchingClient<D>
{
    fn run_stitching_client(
        self,
        proposal_data_hash: B256,
        oracle: Arc<O>,
        stream: Arc<O>,
        beacon: B,
        fpvm_image_id: B256,
        payout_recipient_address: Address,
        stitched_executions: Vec<Vec<Execution>>,
        derivation_cache: Option<CachedDriver>,
        derivation_trace: bool,
        stitched_preconditions: Vec<Precondition>,
        stitched_boot_info: Vec<StitchedBootInfo>,
        chunk_witness: Option<ChunkWitnessData>,
        chunks: Vec<Vec<PartialExecution>>,
    ) -> (BootInfo, ProofJournal, Precondition)
    where
        <B as BlobProvider>::Error: Debug,
    {
        // Queue up precomputed executions
        let (stitched_executions, execution_cache) = split_executions(stitched_executions);

        // Capture chunks for post-core stitching verification. `run_core_client` consumes
        // its own copy when populating `ChunkingEvmFactory`, so we clone here to retain
        // the pre-execution layout for hash-chain verification below.
        let chunks_for_stitching = chunks.clone();

        // Attempt to recompute the output hash at the target block number using kona
        log("RUN");
        let (boot, precondition) = crate::client::core::run_core_client(
            proposal_data_hash,
            oracle,
            stream.clone(),
            beacon,
            self.0,
            execution_cache,
            None,
            derivation_cache,
            derivation_trace.then(Default::default),
            chunk_witness,
            chunks,
            None,
        )
        .expect("Failed to compute output hash.");

        // Verify proofs recursively for boundless composition
        #[cfg(target_os = "zkvm")]
        let proven_fpvm_journals = load_stitching_journals(fpvm_image_id);

        // Stitch recursively composed chunk aggregation proofs
        stitch_chunks(
            &boot,
            fpvm_image_id,
            payout_recipient_address,
            &chunks_for_stitching,
            #[cfg(target_os = "zkvm")]
            &proven_fpvm_journals,
        );

        // Stitch recursively composed execution-only proofs
        stitch_executions(
            &boot,
            fpvm_image_id,
            payout_recipient_address,
            &stitched_executions,
            #[cfg(target_os = "zkvm")]
            &proven_fpvm_journals,
        );

        // Stitch recursively composed proofs
        kona_proof::block_on(stitch_boot_info(
            Some(stream),
            boot,
            fpvm_image_id,
            payout_recipient_address,
            precondition,
            stitched_preconditions,
            stitched_boot_info,
            #[cfg(target_os = "zkvm")]
            &proven_fpvm_journals,
        ))
        .expect("Failed to stitch boot info.")
    }
}

/// Loads and verifies stitching journals for a given FPVM image.
///
/// This function continuously reads receipts representing the proofs of computations from the
/// standard input (stdin). Each receipt is validated against the provided `fpvm_image_id`,
/// representing the image digest of the FPVM. Validated receipts' journal digests are stored
/// in a `HashSet` ensuring uniqueness. If deserialization of the receipt fails, the function
/// terminates and returns the set of proven journal digests.
///
/// # Parameters
/// - `fpvm_image_id`: A `B256` type identifier representing the hashed image ID of the FPVM.
///
/// # Returns
/// - A `HashSet<Digest>` containing the unique journal digests of all verified receipts.
///
/// # Behavior
/// 1. Converts the `fpvm_image_id` into a `Digest` for verification purposes.
/// 2. Reads receipts in a loop from the standard input until an `Err` occurs during deserialization.
///    - While reading receipts:
///      - Logs the verification process.
///      - Deserializes and verifies receipts against the provided `fpvm_image_id`.
///      - Inserts successfully verified journal digests into the `HashSet`.
/// 3. Logs the total number of successfully verified journal digests and exits with the result.
///
/// # Panics
/// Panics if:
/// - Receipt verification fails, indicating an invalid or tampered proof. The panic message will
///   include which journal digest's verification failed.
///
/// # Logging
/// - Logs "VERIFY" at the start of the method.
/// - Logs "VERIFY {journal_digest}" after calculating journal digests.
/// - Logs "PROOFS {count}" denoting the number of proven journal digests before exiting.
///
/// # Notes
/// - The `Receipt::deserialize` and `risc0_zkvm::guest::env::stdin` are used to process input
///   receipts.
/// - This function is designed for environments where proofs generated externally are verified
///   within the FPVM.
#[cfg(target_os = "zkvm")]
pub fn load_stitching_journals(fpvm_image_id: B256) -> HashSet<Digest> {
    log("VERIFY");

    let fpvm_image_id = Digest::from(fpvm_image_id.0);
    let mut proven_fpvm_journals = HashSet::with_hasher(Default::default());

    loop {
        let Ok(receipt) =
            Receipt::deserialize(&mut Deserializer::new(risc0_zkvm::guest::env::stdin()))
        else {
            log(&format!("PROOFS {}", proven_fpvm_journals.len()));
            break proven_fpvm_journals;
        };

        let journal_digest = receipt.journal.digest();
        log(&format!("VERIFY {journal_digest}"));

        // Validate RISC Zero receipts natively
        receipt
            .verify(fpvm_image_id)
            .expect("Failed to verify receipt for {journal_digest}.");

        proven_fpvm_journals.insert(journal_digest);
    }
}

/// Verifies the stitching journal of an FPVM image.
///
/// This function checks the validity of a journal based on its digest and the existing
/// set of proven FPVM journal digests. The behavior of this function depends on the
/// target OS being `zkvm`. If the journal's digest exists in the set of verified digests,
/// it logs that the digest was found. Otherwise, it assumes the journal and attempts to
/// verify it using the RISC Zero ZKVM environment.
///
/// # Parameters
/// - `_fpvm_image_id`: The ID of the FPVM image represented as a `B256` hash. This
///   ID is used during the journal verification process.
/// - `_proof_journal`: The serialized proof journal as a `Vec<u8>`. It serves as
///   the data to be verified.
/// - `proven_fpvm_journals`: A reference to a `HashSet` of digests (of type `Digest`)
///   containing the previously verified journals. This parameter is only used when
///   the target OS is `zkvm`.
///
/// # Logs
/// - Logs a message indicating whether the given journal digest was "FOUND" in the proven
///   set or "ASSUME" if it is not present.
///
/// # Panics
/// - If the verification process fails (i.e., the journal does not match the
///   expected criteria for verification), the function will panic with the message:
///   `"Failed to verify stitched journal assumption"`.
pub fn verify_stitching_journal(
    _fpvm_image_id: B256,
    _proof_journal: Vec<u8>,
    #[cfg(target_os = "zkvm")] proven_fpvm_journals: &HashSet<Digest>,
) {
    #[cfg(target_os = "zkvm")]
    {
        let journal_digest = _proof_journal.digest();
        if proven_fpvm_journals.contains(&journal_digest) {
            crate::client::log(&format!("FOUND {journal_digest}"));
        } else {
            crate::client::log(&format!("ASSUME {journal_digest}"));
            risc0_zkvm::guest::env::verify(_fpvm_image_id.0, &_proof_journal)
                .expect("Failed to verify stitched journal assumption");
        }
    }
}

/// Splits a provided two-dimensional vector of `Execution` objects into two separate structures:
/// - A nested two-dimensional vector where each inner `Execution` is wrapped in an `Arc`.
/// - A flattened vector containing all the `Execution` objects, each wrapped in an `Arc`.
///
/// This function is useful for scenarios where you want to maintain the original structure
/// but also need a separate flattened cache to quickly access all `Execution` objects.
///
/// # Arguments
///
/// * `stitched_executions` - A two-dimensional vector of `Execution` objects (`Vec<Vec<Execution>>`)
///   representing grouped and stitched executions.
///
/// # Returns
///
/// A tuple containing:
/// 1. A two-dimensional vector (`Vec<Vec<Arc<Execution>>>`) where each `Execution` is wrapped in an `Arc`.
/// 2. A flattened vector (`Vec<Arc<Execution>>`) representing a cache of all `Execution` objects.
pub fn split_executions(
    stitched_executions: Vec<Vec<Execution>>,
) -> (Vec<Vec<Arc<Execution>>>, Vec<Arc<Execution>>) {
    let stitched_executions = stitched_executions
        .into_iter()
        .map(|trace| trace.into_iter().map(Arc::new).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let execution_cache = stitched_executions
        .iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    (stitched_executions, execution_cache)
}

/// Stitches a collection of execution traces into a cohesive proof journal and validates the results.
/// This function ensures the integrity of execution traces and their compliance with the rollup configuration.
///
/// # Parameters
/// - `boot`: A reference to the `BootInfo` structure containing the rollup's configuration and state information.
/// - `fpvm_image_id`: The unique identifier of the FPVM (Fault-Proof Virtual Machine) image being used for proofs.
/// - `payout_recipient_address`: The address to receive the payout as a result of the execution.
/// - `stitched_executions`: A reference to a vector of vectors containing execution traces. Each inner vector represents
///   a sequence of linked execution steps (`Execution` objects).
/// - `proven_fpvm_journals` (*conditional*): A reference to a set of `Digest` values representing proven
///   journals from the FPVM. Only available when compiled for `zkvm` target (`#[cfg(target_os = "zkvm")]`).
///
/// # Behavior
/// - When the `boot.l1_head` is zero, it represents a special case where only one batch of execution is validated
///   by the Kailua client. If more than one batch is found, the function panics.
/// - Validates the `receipts_root` of each execution in all traces by comparing it with the computed root value
///   based on the execution result, rollup configuration, and payload attributes' timestamp.
/// - Constructs an expected proof journal for each execution trace, which includes precondition and configuration
///   hashes, and other state values derived from the execution trace (e.g., output roots and block numbers).
/// - When the system is targeting `zkvm`, the proof journal is verified using the `proven_fpvm_journals`.
///
/// # Panics
/// - When `boot.l1_head` is zero but the number of `stitched_executions` exceeds 1.
/// - When an execution trace is empty (used in `.first()` or `.last()` calls without valid elements).
pub fn stitch_executions(
    boot: &BootInfo,
    fpvm_image_id: B256,
    payout_recipient_address: Address,
    stitched_executions: &Vec<Vec<Arc<Execution>>>,
    #[cfg(target_os = "zkvm")] proven_fpvm_journals: &HashSet<Digest>,
) {
    let config_hash = crate::config::config_hash(&boot.rollup_config, &boot.l1_config);
    // When running an execution-only proof, we may only have one batch validated by the kailua client
    if boot.l1_head.is_zero() {
        assert_eq!(1, stitched_executions.len());
        return;
    };
    // Otherwise, we validate that all cached executions have corresponding exec-only proofs
    for execution_trace in stitched_executions {
        let precondition_hash =
            crate::precondition::execution::exec_precondition_hash(execution_trace.as_slice());
        // Construct expected proof journal
        let encoded_journal = ProofJournal::new_stitched(
            fpvm_image_id,
            payout_recipient_address,
            precondition_hash,
            B256::from(config_hash),
            &StitchedBootInfo {
                l1_head: B256::ZERO,
                agreed_l2_output_root: execution_trace
                    .first()
                    .expect("Empty execution trace")
                    .agreed_output,
                claimed_l2_output_root: execution_trace
                    .last()
                    .expect("Empty execution trace")
                    .claimed_output,
                claimed_l2_block_number: execution_trace
                    .last()
                    .expect("Empty execution trace")
                    .artifacts
                    .header
                    .number,
            },
        )
        .encode_packed();
        // Require an execution-only proof for the entire batch
        verify_stitching_journal(
            fpvm_image_id,
            encoded_journal,
            #[cfg(target_os = "zkvm")]
            proven_fpvm_journals,
        )
    }
}

/// Sentinel `l1_head` value identifying chunk-proof journals (see `core.rs` chunk branch).
const CHUNK_SENTINEL_L1_HEAD: B256 = B256::new([0xFF; 32]);

/// Stitches recursively-composed chunk aggregation proofs into the proof journal.
///
/// For each block with at least one chunk, this function:
///   1. Computes each chunk's `chunk_trace` from its committed hashes.
///   2. Constructs the expected [`ProofJournal`] for that chunk, using the chunk's own
///      `agreed_l2_output_root` and `block_env.number - 1` (per-block context stored in
///      each `Chunk` — the outer run's final `BootInfo` cannot be used because its
///      `agreed_l2_output_root` and `claimed_l2_block_number` correspond only to the
///      run's last block, not to each chunked block individually).
///   3. Calls [`verify_stitching_journal`] so the guest either finds the journal in the
///      pre-proven set (FOUND) or assumes it via `env::verify()` (ASSUME).
///
/// # Binding of `Chunk.results` to the authenticated proof
///
/// The chunk proof commits to `chunk_trace = SHA256(tx_hash || pre_db_hash ||
/// post_db_hash || pre_evm_hash || post_evm_hash || results_hash)`, where
/// `results_hash = hash_results(results)` is the canonical SHA256 of the per-tx
/// execution trace (see [`crate::precondition::chunking::hash_results`]). The
/// chunk guest captures its own `ResultAndState` sequence via a tracing EVM
/// wrapper during chunk execution and folds `results_hash` into its journal.
///
/// On the aggregation side, this function recomputes `hash_results(&chunk.results)`
/// and feeds it into the same `compute_chunk_trace` formula to rebuild the expected
/// chunk journal. Any tampering with `chunk.results` — reordering entries,
/// substituting a different-but-endpoint-equivalent trace, altering per-tx gas /
/// logs / state — produces a different `results_hash`, a different `chunk_trace`,
/// and a different journal. `env::verify()` then fails, rejecting the chunk proof's
/// assumption in the aggregation guest.
///
/// With this binding in place, `ChunkingEvm` can safely replay `chunk.results`
/// verbatim: the six-input `chunk_trace` authenticates the exact per-transaction
/// execution trace, not merely the pre→post state endpoints.
///
/// # Panics
///
/// * If [`verify_stitching_journal`] rejects any chunk journal.
pub fn stitch_chunks(
    boot: &BootInfo,
    fpvm_image_id: B256,
    payout_recipient_address: Address,
    chunks_per_block: &[Vec<PartialExecution>],
    #[cfg(target_os = "zkvm")] proven_fpvm_journals: &HashSet<Digest>,
) {
    // Chunking is only defined for derivation+execution mode (l1_head != 0). For
    // execution-only or chunk-proving modes, chunks must be empty.
    if chunks_per_block.iter().all(|c| c.is_empty()) {
        return;
    }

    let config_hash = crate::config::config_hash(&boot.rollup_config, &boot.l1_config);

    for block_chunks in chunks_per_block {
        if block_chunks.is_empty() {
            continue;
        }

        // Verify each chunk's journal.
        for chunk in block_chunks {
            let results_hash = hash_results(&chunk.tx_hashes, &chunk.results);
            let block_ctx_hash = hash_block_ctx(&chunk.block_env, &chunk.op_block_ctx);
            let chunk_trace = compute_chunk_trace(results_hash, block_ctx_hash);
            let precondition_hash =
                B256::new(Precondition::default().chunk(chunk_trace).digest().into());

            // Chunk proofs: agreed == claimed L2 output root (no L2 advancement);
            // block number is the parent block number (one below the executed block,
            // which `chunk.block_env.number` commits to).
            let stitched_boot = StitchedBootInfo {
                l1_head: CHUNK_SENTINEL_L1_HEAD,
                agreed_l2_output_root: chunk.op_block_ctx.parent_hash,
                claimed_l2_output_root: chunk.op_block_ctx.parent_hash,
                claimed_l2_block_number: chunk.block_env.number.to::<u64>().saturating_sub(1),
            };

            let encoded_journal = ProofJournal::new_stitched(
                fpvm_image_id,
                payout_recipient_address,
                precondition_hash,
                B256::from(config_hash),
                &stitched_boot,
            )
            .encode_packed();

            verify_stitching_journal(
                fpvm_image_id,
                encoded_journal,
                #[cfg(target_os = "zkvm")]
                proven_fpvm_journals,
            );
        }
    }
}

/// Stitches multiple boot information records into a unified `ProofJournal`.
///
/// This function consolidates and verifies multiple bootstrapping records, validating their
/// integrity and creating a coherent journal that reflects the intermediate states and outputs
/// of the bootstrapping process.
///
/// NOTE: This method does not support combining execution-only proofs.
///
/// # Arguments
///
/// * `boot` - A reference to the base `BootInfo` structure used as the initial data point.
/// * `fpvm_image_id` - A 256-bit identifier representing the FPVM image being used.
/// * `payout_recipient_address` - The Ethereum address to which payouts should be sent.
/// * `precondition_hash` - A 256-bit hash representing the preconditions required for stitching.
/// * `stitched_boot_info` - A vector of `StitchedBootInfo` objects that are incrementally stitched
///   into the `ProofJournal`.
/// * `proven_fpvm_journals` - (Optional, only on `zkvm` platforms) A reference to a set of
///   precomputed and verified FPVM journal digests used for proof verification.
///
/// # Returns
///
/// A `ProofJournal` object that reflects the final stitched state after processing
/// all input records.
///
/// # Panics
///
/// This function will panic in the following scenarios:
///
/// 1. **Equivalence Check Failure**: If the `l1_head` values in the current and stitched boots
///    are inconsistent.
/// 2. **Progress Check Failure**: If there is no progress between the `agreed_l2_output_root` and
///    `claimed_l2_output_root` of a `stitched_boot` object.
/// 3. **Proof Assumption Failure**: If the stitching proof journal fails the `verify_stitching_journal`
///    check.
/// 4. **Non-contiguous Stitching**: If the claimed and agreed L2 output roots cannot be matched
///    in a forward or backward stitching configuration.
/// 5. **Execution-only Records**: If the combination of execution-only boot infos is attempted.
///
/// # Stitching Logic
///
/// 1. The function initializes a `ProofJournal` object using the base `BootInfo` structure and
///    additional parameters.
/// 2. For each `StitchedBootInfo` object in `stitched_boot_info`:
///     - Verify the equivalence of `l1_head`.
///     - Ensure progress is made between `agreed_l2_output_root` and `claimed_l2_output_root`.
///     - Validate the proof associated with the stitching via the `verify_stitching_journal` function.
///     - Perform continuity checks and update the journal in a forward or backward stitching
///       configuration. If stitching is non-contiguous, the function will panic.
///
/// # Platform-specific Behavior
///
/// * On `zkvm` platforms, the function requires access to `proven_fpvm_journals` to verify stitching
///   proofs. On other platforms, the verification step is omitted.
pub async fn stitch_boot_info<O: CommsClient + FlushableCache + Send + Sync + Debug>(
    stream: Option<Arc<O>>,
    boot: BootInfo,
    fpvm_image_id: B256,
    payout_recipient_address: Address,
    mut precondition: Precondition,
    stitched_preconditions: Vec<Precondition>,
    stitched_boot_infos: Vec<StitchedBootInfo>,
    #[cfg(target_os = "zkvm")] proven_fpvm_journals: &HashSet<Digest>,
) -> anyhow::Result<(BootInfo, ProofJournal, Precondition)> {
    // Equal inputs
    assert_eq!(stitched_preconditions.len(), stitched_boot_infos.len());

    // Instantiate oracle-backed providers
    let mut l1_provider = match stream {
        Some(stream) => Some(OracleL1ChainProvider::new(boot.l1_head, stream).await?),
        None => None,
    };

    // Instantiate base proof journal for validating stitched proofs
    let mut journal = ProofJournal::new(
        fpvm_image_id,
        payout_recipient_address,
        B256::ZERO, // Precondition digest will be finalized below
        &boot,
    );

    // Stitch boot info instances
    let mut l1_head_number = match l1_provider.as_mut() {
        Some(provider) if !boot.l1_head.is_zero() => Some(
            provider
                .header_by_hash(boot.l1_head)
                .await
                .context("boot header_by_hash")?
                .number,
        ),
        _ => None,
    };
    for (stitched_boot, stitched_precondition) in zip(stitched_boot_infos, stitched_preconditions) {
        // Check if stitched l1 head is in the same chain
        if boot.l1_head.is_zero() || stitched_boot.l1_head.is_zero() {
            unimplemented!("Stitching boot infos of execution-only proofs is not supported.");
        } else if let Some(l1_provider) = l1_provider.as_mut() {
            // Retrieve the full header, which must then be verified to be from the same chain
            let stitched_l1_header = l1_provider
                .header_by_hash(stitched_boot.l1_head)
                .await
                .context("header_by_hash")?;
            // Ensure non-increasing derivation heads
            let l1_head_number = l1_head_number.as_mut().unwrap();
            assert!(stitched_l1_header.number <= *l1_head_number);
            *l1_head_number = stitched_l1_header.number;
            // Ensure that querying the oracle by the header number yields the same header hash
            assert_eq!(
                l1_provider
                    .block_info_by_number(stitched_l1_header.number)
                    .await
                    .context("block_info_by_number")?
                    .hash,
                stitched_boot.l1_head
            );
        }
        // Require equivalence in proposal precondition
        assert_eq!(
            precondition.proposal_blobs,
            stitched_precondition.proposal_blobs
        );
        // Require backward stitching (stitched proof leads to current journal state)
        assert_eq!(
            stitched_boot.claimed_l2_output_root,
            journal.agreed_l2_output_root
        );
        // Stitched boot's trace must be our cache
        assert_eq!(
            precondition.derivation_cache,
            stitched_precondition.derivation_trace
        );
        // Update our initial l2 output root to that of the stitched boot
        journal.agreed_l2_output_root = stitched_boot.agreed_l2_output_root;
        // Update our cache to be that of the backwards stitched boot
        precondition.derivation_cache = stitched_precondition.derivation_cache;
        // Require derivation proof for stitched boot
        verify_stitching_journal(
            fpvm_image_id,
            ProofJournal::new_stitched(
                fpvm_image_id,
                payout_recipient_address,
                B256::new(stitched_precondition.digest().into()),
                journal.config_hash,
                &stitched_boot,
            )
            .encode_packed(),
            #[cfg(target_os = "zkvm")]
            proven_fpvm_journals,
        );
    }

    // Update the final precondition hash
    journal.precondition_hash = B256::new(precondition.digest().into());

    // Report final precondition
    log("STITCHED");
    log(&format!("{journal:?}"));
    log(&format!("{precondition:?}"));

    Ok((boot, journal, precondition))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod tests {
    use super::*;
    use crate::client::core::tests::test_derivation;
    use crate::client::core::EthereumDataSourceProvider;
    use crate::client::tests::TestOracle;
    use crate::precondition::proposal::ProposalPrecondition;
    use alloy_primitives::b256;
    use anyhow::Context;
    use kona_proof::l1::OracleBlobProvider;
    use rayon::prelude::{IntoParallelIterator, ParallelIterator};
    use std::iter::repeat_n;

    fn setup() {
        let _ = kona_cli::LogConfig::new(kona_cli::LogArgs {
            level: 1,
            stdout_quiet: false,
            stdout_format: Default::default(),
            file_directory: None,
            file_format: Default::default(),
            file_rotation: Default::default(),
        })
        .init_tracing_subscriber(None);
    }

    fn teardown() {
        let _ = kona_cli::LogConfig::new(kona_cli::LogArgs {
            level: 0,
            stdout_quiet: false,
            stdout_format: Default::default(),
            file_directory: None,
            file_format: Default::default(),
            file_rotation: Default::default(),
        })
        .init_tracing_subscriber(None);
    }

    fn validate_proof_journal(
        proof_journal: ProofJournal,
        boot_info: BootInfo,
        precondition_hash: Option<B256>,
    ) {
        assert_eq!(proof_journal.l1_head, boot_info.l1_head);
        assert_eq!(
            proof_journal.agreed_l2_output_root,
            boot_info.agreed_l2_output_root
        );
        assert_eq!(
            proof_journal.claimed_l2_output_root,
            boot_info.claimed_l2_output_root
        );
        assert_eq!(
            proof_journal.claimed_l2_block_number,
            boot_info.claimed_l2_block_number
        );
        if let Some(expected_precondition_hash) = precondition_hash {
            assert_eq!(proof_journal.precondition_hash, expected_precondition_hash);
        }
        assert!(proof_journal.payout_recipient.is_zero());
        assert!(proof_journal.fpvm_image_id.is_zero());
    }

    pub fn test_stitching(
        boot_info: BootInfo,
        precondition_validation_data: Option<ProposalPrecondition>,
        stitched_executions: Vec<Vec<Execution>>,
        derivation_cache: Option<CachedDriver>,
        derivation_trace: bool,
        stitched_preconditions: Vec<Precondition>,
        stitched_boot_info: Vec<StitchedBootInfo>,
    ) {
        let precondition_hash = precondition_validation_data
            .as_ref()
            .map(|d| d.precondition_hash());
        let proof_journal = test_stitching_client(
            boot_info.clone(),
            precondition_validation_data,
            stitched_executions,
            derivation_cache,
            derivation_trace,
            stitched_preconditions,
            stitched_boot_info,
        );
        validate_proof_journal(proof_journal, boot_info, precondition_hash);
    }

    pub fn test_stitching_client(
        boot_info: BootInfo,
        proposal_precondition: Option<ProposalPrecondition>,
        stitched_executions: Vec<Vec<Execution>>,
        derivation_cache: Option<CachedDriver>,
        derivation_trace: bool,
        stitched_preconditions: Vec<Precondition>,
        stitched_boot_info: Vec<StitchedBootInfo>,
    ) -> ProofJournal {
        let oracle = Arc::new(TestOracle::new(boot_info.clone()));
        let precondition_validation_data_hash = match proposal_precondition {
            None => B256::ZERO,
            Some(data) => oracle.add_precondition_data(data),
        };
        KonaStitchingClient(EthereumDataSourceProvider)
            .run_stitching_client(
                precondition_validation_data_hash,
                oracle.clone(),
                oracle.clone(),
                OracleBlobProvider::new(oracle.clone()),
                B256::ZERO,
                Address::ZERO,
                stitched_executions,
                derivation_cache,
                derivation_trace,
                stitched_preconditions,
                stitched_boot_info,
                None,
                Vec::new(),
            )
            .1
    }

    pub fn test_stitching_boots(
        boot_info: BootInfo,
        precondition_validation_data: Option<ProposalPrecondition>,
    ) -> anyhow::Result<()> {
        let stitched_executions = test_derivation(
            boot_info.clone(),
            precondition_validation_data.clone(),
            None,
            None,
        )
        .context("test_derivation")?
        .into_iter()
        .map(|e| e.as_ref().clone())
        .collect::<Vec<_>>();
        let stitched_boot_info = stitched_executions
            .iter()
            .map(|e| StitchedBootInfo {
                l1_head: boot_info.l1_head,
                agreed_l2_output_root: e.agreed_output,
                claimed_l2_output_root: e.claimed_output,
                claimed_l2_block_number: e.artifacts.header.number,
            })
            .collect::<Vec<_>>();
        let precondition_hash = precondition_validation_data
            .as_ref()
            .map(|d| d.precondition_hash());
        let stitched_preconditions = repeat_n(
            Precondition::default().proposal(precondition_hash.unwrap_or_default()),
            stitched_boot_info.len(),
        )
        .collect::<Vec<_>>();
        // backward stitching pass
        let ending_block_number = stitched_executions
            .last()
            .map(|e| e.artifacts.header.number)
            .unwrap_or(boot_info.claimed_l2_block_number);
        let proof_journal = test_stitching_client(
            BootInfo {
                l1_head: boot_info.l1_head,
                agreed_l2_output_root: boot_info.claimed_l2_output_root,
                claimed_l2_output_root: boot_info.claimed_l2_output_root,
                claimed_l2_block_number: ending_block_number,
                chain_id: boot_info.chain_id,
                rollup_config: boot_info.rollup_config.clone(),
                l1_config: boot_info.l1_config.clone(),
            },
            precondition_validation_data.clone(),
            vec![],
            None,
            false,
            stitched_preconditions.clone().into_iter().rev().collect(),
            stitched_boot_info.clone().into_iter().rev().collect(),
        );
        validate_proof_journal(proof_journal, boot_info.clone(), precondition_hash);
        // fail out of order stitching
        let n = stitched_executions.len();
        (0..n).into_par_iter().for_each(|i| {
            (i + 1..n).into_par_iter().for_each(|j| {
                let mut stitched_preconditions = stitched_preconditions.clone();
                let mut stitched_boot_info = stitched_boot_info.clone();
                stitched_boot_info.swap(i, j);
                stitched_preconditions.swap(i, j);
                let result = std::panic::catch_unwind(|| {
                    test_stitching_client(
                        BootInfo {
                            l1_head: boot_info.l1_head,
                            agreed_l2_output_root: boot_info.claimed_l2_output_root,
                            claimed_l2_output_root: boot_info.claimed_l2_output_root,
                            claimed_l2_block_number: ending_block_number,
                            chain_id: boot_info.chain_id,
                            rollup_config: boot_info.rollup_config.clone(),
                            l1_config: boot_info.l1_config.clone(),
                        },
                        precondition_validation_data.clone(),
                        vec![],
                        None,
                        false,
                        stitched_preconditions.clone().into_iter().rev().collect(),
                        stitched_boot_info.clone().into_iter().rev().collect(),
                    )
                });
                assert!(result.is_err());
            })
        });

        Ok(())
    }

    pub fn test_stitching_executions(
        boot_info: BootInfo,
        precondition_validation_data: Option<ProposalPrecondition>,
    ) -> anyhow::Result<()> {
        let stitched_executions = test_derivation(
            boot_info.clone(),
            precondition_validation_data.clone(),
            None,
            None,
        )
        .context("test_derivation")?
        .into_iter()
        .map(|e| e.as_ref().clone())
        .collect::<Vec<_>>();
        // flat pass
        test_stitching(
            boot_info.clone(),
            precondition_validation_data.clone(),
            vec![stitched_executions.clone()],
            None,
            false,
            vec![],
            vec![],
        );
        let n = stitched_executions.len();
        // don't test exec trace stitching if unnecessary or exec only mode
        if n == 1 {
            return Ok(());
        }
        // split pass
        let (left, right) = stitched_executions.split_at(n / 2);
        test_stitching(
            boot_info.clone(),
            precondition_validation_data.clone(),
            vec![left.to_vec(), right.to_vec()],
            None,
            false,
            vec![],
            vec![],
        );
        // fully fragmented pass
        test_stitching(
            boot_info.clone(),
            precondition_validation_data.clone(),
            stitched_executions.into_iter().map(|e| vec![e]).collect(),
            None,
            false,
            vec![],
            vec![],
        );
        Ok(())
    }

    pub fn test_stitching_execution_only(
        mut boot_info: BootInfo,
        precondition_validation_data: Option<ProposalPrecondition>,
        stitched_preconditions: Vec<Precondition>,
        stitched_boot_info: Vec<StitchedBootInfo>,
    ) -> anyhow::Result<()> {
        let stitched_executions = test_derivation(
            boot_info.clone(),
            precondition_validation_data.clone(),
            None,
            None,
        )
        .context("test_derivation")?
        .into_iter()
        .map(|e| e.as_ref().clone())
        .collect::<Vec<_>>();
        // flat pass
        boot_info.l1_head = B256::ZERO;
        test_stitching(
            boot_info.clone(),
            precondition_validation_data.clone(),
            vec![stitched_executions.clone()],
            None,
            false,
            stitched_preconditions,
            stitched_boot_info,
        );
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491250() {
        setup();

        test_stitching(
            BootInfo {
                l1_head: b256!(
                    "0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"
                ),
                agreed_l2_output_root: b256!(
                    "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
                ),
                claimed_l2_output_root: b256!(
                    "0xa130fbfa315391b28668609252e4c09c3df3b77562281b996af30bf056cbb2c1"
                ),
                claimed_l2_block_number: 16491250,
                chain_id: 11155420,
                rollup_config: Default::default(),
                l1_config: Default::default(),
            },
            None,
            vec![],
            None,
            false,
            vec![],
            vec![],
        );

        teardown();
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491250_stitched_execution() {
        setup();

        test_stitching_executions(
            BootInfo {
                l1_head: b256!(
                    "0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"
                ),
                agreed_l2_output_root: b256!(
                    "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
                ),
                claimed_l2_output_root: b256!(
                    "0xa130fbfa315391b28668609252e4c09c3df3b77562281b996af30bf056cbb2c1"
                ),
                claimed_l2_block_number: 16491250,
                chain_id: 11155420,
                rollup_config: Default::default(),
                l1_config: Default::default(),
            },
            None,
        )
        .unwrap();

        teardown();
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491349() {
        setup();

        test_stitching(
            BootInfo {
                l1_head: b256!(
                    "0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"
                ),
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
            },
            Some(ProposalPrecondition {
                proposal_l2_head_number: 16491249,
                proposal_output_count: 1,
                output_block_span: 100,
                blob_hashes: vec![],
            }),
            vec![],
            None,
            false,
            vec![],
            vec![],
        );

        teardown();
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491349_stitched_executions() {
        setup();

        test_stitching_executions(
            BootInfo {
                l1_head: b256!(
                    "0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"
                ),
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
            },
            Some(ProposalPrecondition {
                proposal_l2_head_number: 16491249,
                proposal_output_count: 1,
                output_block_span: 100,
                blob_hashes: vec![],
            }),
        )
        .unwrap();

        teardown();
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491349_execution_only() {
        setup();

        test_stitching_execution_only(
            BootInfo {
                l1_head: b256!(
                    "0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"
                ),
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
            },
            None,
            vec![],
            vec![],
        )
        .unwrap();

        teardown();
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491349_stitched_boots() {
        setup();

        test_stitching_boots(
            BootInfo {
                l1_head: b256!(
                    "0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"
                ),
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
            },
            Some(ProposalPrecondition {
                proposal_l2_head_number: 16491249,
                proposal_output_count: 1,
                output_block_span: 100,
                blob_hashes: vec![],
            }),
        )
        .unwrap();

        teardown();
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_stitch_boot_info_no_stream() {
        // Exercises the `None => None` arm of the `stream` match (and the
        // corresponding `_ => None` arm of the `l1_head_number` match) when the
        // host is assumed to handle l1-head chain continuity off-stream —
        // matches the `None` call in `prover::tasks::stitch_boot_info`.
        let boot_info = BootInfo {
            l1_head: Default::default(),
            agreed_l2_output_root: Default::default(),
            claimed_l2_output_root: Default::default(),
            claimed_l2_block_number: 0,
            chain_id: 0,
            rollup_config: Default::default(),
            l1_config: Default::default(),
        };
        let (_boot, _journal, _precondition) = stitch_boot_info::<crate::oracle::vec::VecOracle>(
            None,
            boot_info,
            B256::ZERO,
            Address::ZERO,
            Precondition::default(),
            vec![],
            vec![],
        )
        .await
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    #[should_panic(expected = "execution-only proofs")]
    pub async fn test_stitch_boot_info_execution_only_panics() {
        // Exercises the `unimplemented!()` guard that rejects stitching of
        // execution-only proofs (either boot.l1_head or stitched.l1_head == 0).
        let boot_info = BootInfo {
            l1_head: b256!("0x1111111111111111111111111111111111111111111111111111111111111111"),
            agreed_l2_output_root: Default::default(),
            claimed_l2_output_root: Default::default(),
            claimed_l2_block_number: 0,
            chain_id: 0,
            rollup_config: Default::default(),
            l1_config: Default::default(),
        };
        let stitched_boot = StitchedBootInfo {
            l1_head: B256::ZERO,
            agreed_l2_output_root: Default::default(),
            claimed_l2_output_root: Default::default(),
            claimed_l2_block_number: 0,
        };
        let _ = stitch_boot_info::<crate::oracle::vec::VecOracle>(
            None,
            boot_info,
            B256::ZERO,
            Address::ZERO,
            Precondition::default(),
            vec![Precondition::default()],
            vec![stitched_boot],
        )
        .await;
    }

    // -- stitch_chunks tests --

    use alloy_primitives::keccak256;

    /// Minimal BootInfo for stitch_chunks tests — derivation+execution mode (`l1_head`
    /// non-zero). Values do not need to correspond to a real chain; `stitch_chunks`
    /// only inspects the BootInfo's `agreed_l2_output_root`, `claimed_l2_block_number`,
    /// and config-hash-derived fields.
    fn chunks_boot_info() -> BootInfo {
        BootInfo {
            l1_head: b256!("0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
            agreed_l2_output_root: b256!(
                "0xBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB"
            ),
            claimed_l2_output_root: b256!(
                "0xCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC"
            ),
            claimed_l2_block_number: 42,
            chain_id: 1,
            rollup_config: Default::default(),
            l1_config: Default::default(),
        }
    }

    fn make_chunk(
        _agreed_db: B256,
        _claimed_db: B256,
        _agreed_evm: B256,
        _claimed_evm: B256,
    ) -> PartialExecution {
        PartialExecution {
            tx_hashes: Vec::new(),
            results: Vec::new(),
            block_env: alloy_evm::revm::context::BlockEnv::default(),
            op_block_ctx: alloy_op_evm::block::OpBlockExecutionCtx::default(),
        }
    }

    /// Empty chunks (no chunk data for any block) → stitch_chunks is a no-op and must
    /// not panic. This is the degenerate case for witnesses with `chunks = vec![]`.
    #[test]
    fn stitch_chunks_empty_is_noop() {
        let boot = chunks_boot_info();
        stitch_chunks(&boot, B256::ZERO, Address::ZERO, &[]);
    }

    /// A single block with a single chunk — no continuity checks are required. The
    /// chunk journal is constructed and (in non-zkvm builds) `verify_stitching_journal`
    /// is a no-op, so the function returns cleanly.
    #[test]
    fn stitch_chunks_single_block_single_chunk() {
        let boot = chunks_boot_info();
        let chunk = make_chunk(
            keccak256("db0"),
            keccak256("db1"),
            keccak256("evm0"),
            keccak256("evm1"),
        );
        stitch_chunks(&boot, B256::ZERO, Address::ZERO, &[vec![chunk]]);
    }

    /// Valid multi-chunk hash chain: each chunk's `agreed_db` matches the prior
    /// chunk's `claimed_db`, same for `agreed_evm`/`claimed_evm`. Must not panic.
    #[test]
    fn stitch_chunks_valid_hash_chain() {
        let boot = chunks_boot_info();
        let db0 = keccak256("db_0");
        let db1 = keccak256("db_1");
        let db2 = keccak256("db_2");
        let evm0 = keccak256("evm_0");
        let evm1 = keccak256("evm_1");
        let evm2 = keccak256("evm_2");
        let chunks = vec![vec![
            make_chunk(db0, db1, evm0, evm1),
            make_chunk(db1, db2, evm1, evm2),
        ]];
        stitch_chunks(&boot, B256::ZERO, Address::ZERO, &chunks);
    }

    /// Binding: altering `chunk.results` must change the reconstructed `chunk_trace`.
    ///
    /// This is the linchpin of finding #1's fix — the aggregation's reconstructed
    /// journal depends on `hash_results(&chunk.results)` via `compute_chunk_trace`, so
    /// any altered results vec produces a different journal digest. An adversary who
    /// swaps `results` to a different-but-state-endpoint-equivalent trace would
    /// generate a different `chunk_trace`, fail `env::verify()` on the swapped journal,
    /// and the aggregation proof cannot produce a valid witness.
    ///
    /// This test exercises the non-zkvm branch (`verify_stitching_journal` is a no-op
    /// there) but verifies structurally that the reconstructed chunk_trace diverges
    /// when `chunk.results` is altered — the same computation feeds journal construction
    /// on the zkvm side.
    #[test]
    fn stitch_chunks_results_tampering_changes_chunk_trace() {
        use crate::precondition::chunking::{compute_chunk_trace, hash_block_ctx, hash_results};
        use alloy_evm::op_revm::OpHaltReason;
        use alloy_evm::revm::context_interface::result::{
            ExecutionResult, Output, ResultAndState, SuccessReason,
        };

        // Two chunks identical except for a single gas_used value in `results`.
        let stub = |gas: u64| ResultAndState::<OpHaltReason> {
            result: ExecutionResult::Success {
                reason: SuccessReason::Return,
                gas_used: gas,
                gas_refunded: 0,
                logs: vec![],
                output: Output::Call(alloy_primitives::Bytes::new()),
            },
            state: Default::default(),
        };

        let mut chunk_a = make_chunk(
            keccak256("db0"),
            keccak256("db1"),
            keccak256("evm0"),
            keccak256("evm1"),
        );
        chunk_a.tx_hashes = vec![B256::ZERO];
        chunk_a.results = vec![stub(21000)];

        let mut chunk_b = chunk_a.clone();
        chunk_b.results = vec![stub(21001)]; // tampered!

        let trace_a = compute_chunk_trace(
            hash_results(&chunk_a.tx_hashes, &chunk_a.results),
            hash_block_ctx(&chunk_a.block_env, &chunk_a.op_block_ctx),
        );
        let trace_b = compute_chunk_trace(
            hash_results(&chunk_b.tx_hashes, &chunk_b.results),
            hash_block_ctx(&chunk_b.block_env, &chunk_b.op_block_ctx),
        );
        assert_ne!(
            trace_a, trace_b,
            "tampering with chunk.results must change chunk_trace (binding invariant)"
        );
    }

    /// Per-block context: in a multi-block derivation run, each block's chunks carry
    /// their own `agreed_l2_output_root` and `block_env.number` (which drives the
    /// journal's `claimed_l2_block_number = block_env.number - 1`). These must flow
    /// into the constructed chunk journal — not the outer `BootInfo`'s (final) values.
    ///
    /// This test does not validate the journal bytes (that would require zkvm-side
    /// `env::verify`), but it exercises the two-block path and relies on
    /// `verify_stitching_journal` being a no-op in non-zkvm builds. Any panic would
    /// indicate a regression in the per-block context plumbing.
    #[test]
    fn stitch_chunks_multi_block_uses_per_chunk_context() {
        use alloy_primitives::U256;
        let boot = chunks_boot_info();
        // Block A: block 11 (parent = 10), agreed output root X.
        let mut chunk_a = make_chunk(
            keccak256("db_a0"),
            keccak256("db_a1"),
            keccak256("evm_a0"),
            keccak256("evm_a1"),
        );
        chunk_a.block_env.number = U256::from(11u64);

        // Block B: block 12 (parent = 11), agreed output root Y (different from X, and
        // different from boot.agreed_l2_output_root). If stitch_chunks incorrectly
        // substituted the outer BootInfo values, the journal for this chunk would
        // refer to boot.agreed_l2_output_root and boot.claimed_l2_block_number
        // regardless of the per-chunk values we set here.
        let mut chunk_b = make_chunk(
            keccak256("db_b0"),
            keccak256("db_b1"),
            keccak256("evm_b0"),
            keccak256("evm_b1"),
        );
        chunk_b.block_env.number = U256::from(12u64);

        stitch_chunks(
            &boot,
            B256::ZERO,
            Address::ZERO,
            &[vec![chunk_a], vec![chunk_b]],
        );
    }

    /// Blocks with empty chunk vecs are skipped: a witness that mixes some chunked
    /// blocks and some non-chunked blocks passes intact.
    #[test]
    fn stitch_chunks_empty_block_is_skipped() {
        let boot = chunks_boot_info();
        // Block 0 empty, block 1 has a single chunk, block 2 empty.
        let chunks = vec![
            vec![],
            vec![make_chunk(
                keccak256("db0"),
                keccak256("db1"),
                keccak256("evm0"),
                keccak256("evm1"),
            )],
            vec![],
        ];
        stitch_chunks(&boot, B256::ZERO, Address::ZERO, &chunks);
    }
}

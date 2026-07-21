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

use crate::boot::{L1_HEAD_EXEC_ONLY_SENTINEL, L1_HEAD_SENTINELS, StitchedBootInfo};
use crate::client::core::DASourceProvider;
use crate::client::log;
use crate::config::config_hash;
use crate::driver::CachedDriver;
use crate::executor::Execution;
use crate::journal::ProofJournal;
use crate::kona::OracleL1ChainProvider;
use crate::precondition::Precondition;
#[cfg(feature = "experimental")]
use crate::{
    boot::L1_HEAD_TXN_ONLY_SENTINEL,
    evm::{partial::PartialExecution, witness::PartialExecutionWitness},
};
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
    risc0_zkvm::{Receipt, serde::Deserializer, sha::Digest},
    serde::Deserialize,
};

/// A client that runs the core Kailua program and absorbs other proofs' results into a single
/// journal, implemented once per data availability integration.
pub trait StitchingClient<
    O: CommsClient + FlushableCache + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
>
{
    /// Runs the Kailua client to transition the rollup state, then combines the result with the
    /// other proven contiguous state transitions to yield a single overarching [ProofJournal]
    /// and [Precondition]. Also returns the [BootInfo] loaded by the client.
    ///
    /// # Arguments
    /// * `proposal_data_hash` - The hash referencing the proposal blob precondition data, if any.
    /// * `oracle` - The client for preloaded communication with the host.
    /// * `stream` - The client for streamed communication with the host.
    /// * `beacon` - The blob provider.
    /// * `fpvm_image_id` - The image id against which stitched proofs are verified.
    /// * `payout_recipient_address` - The proof payout recipient.
    /// * `stitched_executions` - Execution traces of the exec-only proofs being stitched.
    /// * `derivation_cache` - A derivation pipeline snapshot to resume from, if any.
    /// * `derivation_trace` - Whether to commit a final pipeline snapshot in the precondition.
    /// * `stitched_preconditions` - The preconditions of the boot claims being stitched.
    /// * `stitched_boot_info` - The boot claims of the other proofs being stitched.
    /// * `pe_witness` - An optional witness for running a partial execution.
    /// * `partial_executions` - Partial execution (chunk) results to reuse.
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
        #[cfg(feature = "experimental")] pe_witness: Option<PartialExecutionWitness>,
        #[cfg(feature = "experimental")] partial_executions: Vec<Vec<PartialExecution>>,
    ) -> (BootInfo, ProofJournal, Precondition)
    where
        <B as BlobProvider>::Error: Debug;
}

/// The base [StitchingClient], generic over the data availability source factory used for
/// derivation.
#[derive(Clone, Debug)]
pub struct KonaStitchingClient<D: Clone + Debug>(
    /// The data availability source factory.
    pub D,
);

impl<
    O: CommsClient + FlushableCache + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
    D: DASourceProvider<OracleL1ChainProvider<O>, B> + Clone + Debug,
> StitchingClient<O, B> for KonaStitchingClient<D>
{
    /// Runs [crate::client::core::run_core_client], then stitches partial executions, exec-only
    /// proofs, and boot claims into the resulting journal. On zkVM targets, receipts streamed
    /// from the host are verified natively up front; any stitched journal not among them becomes
    /// a deferred verification assumption.
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
        #[cfg(feature = "experimental")] pe_witness: Option<PartialExecutionWitness>,
        #[cfg(feature = "experimental")] partial_executions: Vec<Vec<PartialExecution>>,
    ) -> (BootInfo, ProofJournal, Precondition)
    where
        <B as BlobProvider>::Error: Debug,
    {
        // Queue up precomputed executions
        let (stitched_executions, execution_cache) = split_executions(stitched_executions);

        // Precompute binding data for stitching partial executions before they are moved
        #[cfg(feature = "experimental")]
        let pe_boots = precompute_pe_boots(&partial_executions);

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
            #[cfg(feature = "experimental")]
            pe_witness,
            #[cfg(feature = "experimental")]
            partial_executions,
            #[cfg(feature = "experimental")]
            None,
        )
        .expect("Failed to compute output hash.");

        // Short-circuit all stitching logic when partial proving
        #[cfg(feature = "experimental")]
        if boot.l1_head == L1_HEAD_TXN_ONLY_SENTINEL {
            let proof_journal = ProofJournal::new(
                fpvm_image_id,
                payout_recipient_address,
                B256::new(precondition.digest().into()),
                &boot,
            );
            return (boot, proof_journal, precondition);
        }

        // Verify proofs recursively for boundless composition
        #[cfg(target_os = "zkvm")]
        let proven_fpvm_journals = load_stitching_journals(fpvm_image_id);

        // Stitch recursively composed partial executions
        #[cfg(feature = "experimental")]
        stitch_partial_executions(
            &boot,
            fpvm_image_id,
            payout_recipient_address,
            pe_boots,
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

/// Reads RISC Zero receipts from the zkVM input stream until it is exhausted, verifying each
/// against the FPVM image id — panicking on failure — and returning the set of proven journal
/// digests.
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

/// Requires the given journal to be proven for the FPVM image: journals verified up front pass
/// immediately, and any other becomes a verification assumption to be discharged through proof
/// composition. No-op outside the zkVM.
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

/// Wraps every execution in an [Arc], returning both the original per-proof grouping and a
/// flattened execution cache sharing the same allocations.
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

/// Requires an execution-only proof covering each stitched execution trace by reconstructing
/// the exact journal such a proof must commit to: the exec-only sentinel L1 head, the trace's
/// execution precondition hash, and the output roots the trace spans. When this proof is itself
/// execution-only, no proofs are required and only the single client-validated trace is allowed.
pub fn stitch_executions(
    boot: &BootInfo,
    fpvm_image_id: B256,
    payout_recipient_address: Address,
    stitched_executions: &Vec<Vec<Arc<Execution>>>,
    #[cfg(target_os = "zkvm")] proven_fpvm_journals: &HashSet<Digest>,
) {
    let config_hash = config_hash(&boot.rollup_config, &boot.l1_config);
    // When running an execution-only proof, we may only have one batch validated by the kailua client
    if boot.l1_head == L1_HEAD_EXEC_ONLY_SENTINEL {
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

/// Precomputes precondition and stitched boot info data for partial executions
#[cfg(feature = "experimental")]
pub fn precompute_pe_boots(
    partial_executions: &[Vec<PartialExecution>],
) -> Vec<(B256, StitchedBootInfo)> {
    let mut result = vec![];

    for block_partials in partial_executions {
        if block_partials.is_empty() {
            continue;
        }

        // Verify each chunk's journal.
        for partial in block_partials {
            // Create required boot info
            let stitched_boot = StitchedBootInfo {
                l1_head: L1_HEAD_TXN_ONLY_SENTINEL,
                agreed_l2_output_root: partial.op_block_ctx.parent_hash,
                claimed_l2_output_root: partial.op_block_ctx.parent_hash,
                claimed_l2_block_number: partial.block_env.number.to::<u64>().saturating_sub(1),
            };

            result.push((partial.precondition_hash(), stitched_boot));
        }
    }

    result
}

/// Stitches recursively-composed partial execution proofs into the proof journal.
#[cfg(feature = "experimental")]
pub fn stitch_partial_executions(
    boot: &BootInfo,
    fpvm_image_id: B256,
    payout_recipient_address: Address,
    pe_boots: Vec<(B256, StitchedBootInfo)>,
    #[cfg(target_os = "zkvm")] proven_fpvm_journals: &HashSet<Digest>,
) {
    let config_hash = config_hash(&boot.rollup_config, &boot.l1_config);

    for (precondition_hash, stitched_boot) in pe_boots {
        // Create journal
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

/// Absorbs the boot claims of other derivation proofs into a unified [ProofJournal], returning
/// the final boot record, journal, and precondition.
///
/// Claims are stitched backwards: each stitched claim must end at the journal's current agreed
/// output root, its derivation trace must equal the current derivation cache, and its proposal
/// blobs condition must match — the journal's agreed root and derivation cache then rewind to
/// the stitched claim's. Each claim requires a matching derivation proof via
/// [verify_stitching_journal], and its L1 head must be an ancestor of the boot L1 head (walked
/// through the header chain, with successive heads non-increasing). Panics on any violation;
/// stitching claims of non-derivation proofs is unsupported.
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
        Some(provider) if !L1_HEAD_SENTINELS.contains(&boot.l1_head) => Some(
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
        if L1_HEAD_SENTINELS.contains(&boot.l1_head)
            || L1_HEAD_SENTINELS.contains(&stitched_boot.l1_head)
        {
            unimplemented!("Stitching boot infos of non-derivation proofs is not supported.");
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
    use crate::client::core::EthereumDataSourceProvider;
    use crate::client::core::tests::{op_sepolia_16491249_16491349, test_derivation};
    use crate::client::tests::TestOracle;
    use crate::precondition::proposal::ProposalPrecondition;
    use alloy_primitives::b256;
    use anyhow::Context;
    use kona_proof::l1::OracleBlobProvider;
    #[cfg(feature = "experimental")]
    use {
        crate::client::core::split_collected_partials,
        crate::client::core::tests::{make_pe_boot, test_derivation_with_partials},
        rayon::prelude::{IntoParallelIterator, ParallelIterator},
        std::iter::repeat_n,
    };

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

    #[allow(clippy::too_many_arguments)]
    pub fn test_stitching(
        boot_info: BootInfo,
        precondition_validation_data: Option<ProposalPrecondition>,
        stitched_executions: Vec<Vec<Execution>>,
        derivation_cache: Option<CachedDriver>,
        derivation_trace: bool,
        stitched_preconditions: Vec<Precondition>,
        stitched_boot_info: Vec<StitchedBootInfo>,
        #[cfg(feature = "experimental")] partial_executions: Vec<Vec<PartialExecution>>,
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
            #[cfg(feature = "experimental")]
            partial_executions,
        );
        validate_proof_journal(proof_journal, boot_info, precondition_hash);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn test_stitching_client(
        boot_info: BootInfo,
        proposal_precondition: Option<ProposalPrecondition>,
        stitched_executions: Vec<Vec<Execution>>,
        derivation_cache: Option<CachedDriver>,
        derivation_trace: bool,
        stitched_preconditions: Vec<Precondition>,
        stitched_boot_info: Vec<StitchedBootInfo>,
        #[cfg(feature = "experimental")] partial_executions: Vec<Vec<PartialExecution>>,
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
                #[cfg(feature = "experimental")]
                None,
                #[cfg(feature = "experimental")]
                partial_executions,
            )
            .1
    }

    #[cfg(feature = "experimental")]
    #[ignore]
    pub async fn test_stitching_boots(
        boot_info: BootInfo,
        precondition_validation_data: Option<ProposalPrecondition>,
        partial_executions: Vec<Vec<PartialExecution>>,
    ) -> anyhow::Result<()> {
        let stitched_executions = test_derivation(
            boot_info.clone(),
            precondition_validation_data.clone(),
            None,
            None,
        )
        .await
        .context("test_derivation")?;
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
            partial_executions,
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
                        vec![],
                    )
                });
                assert!(result.is_err());
            })
        });

        Ok(())
    }

    pub async fn test_stitching_executions(
        boot_info: BootInfo,
        precondition_validation_data: Option<ProposalPrecondition>,
    ) -> anyhow::Result<()> {
        let stitched_executions = test_derivation(
            boot_info.clone(),
            precondition_validation_data.clone(),
            None,
            None,
        )
        .await
        .context("test_derivation")?;
        // flat pass
        test_stitching(
            boot_info.clone(),
            precondition_validation_data.clone(),
            vec![stitched_executions.clone()],
            None,
            false,
            vec![],
            vec![],
            #[cfg(feature = "experimental")]
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
            #[cfg(feature = "experimental")]
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
            #[cfg(feature = "experimental")]
            vec![],
        );
        Ok(())
    }

    pub async fn test_stitching_execution_only(
        mut boot_info: BootInfo,
        precondition_validation_data: Option<ProposalPrecondition>,
        stitched_preconditions: Vec<Precondition>,
        stitched_boot_info: Vec<StitchedBootInfo>,
        #[cfg(feature = "experimental")] partial_executions: Vec<Vec<PartialExecution>>,
    ) -> anyhow::Result<()> {
        let stitched_executions = test_derivation(
            boot_info.clone(),
            precondition_validation_data.clone(),
            None,
            None,
        )
        .await
        .context("test_derivation")?;
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
            #[cfg(feature = "experimental")]
            partial_executions,
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
            #[cfg(feature = "experimental")]
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
        .await
        .unwrap();

        teardown();
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491349() {
        setup();

        test_stitching(
            op_sepolia_16491249_16491349(),
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
            #[cfg(feature = "experimental")]
            vec![],
        );

        teardown();
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491349_stitched_executions() {
        setup();

        test_stitching_executions(
            op_sepolia_16491249_16491349(),
            Some(ProposalPrecondition {
                proposal_l2_head_number: 16491249,
                proposal_output_count: 1,
                output_block_span: 100,
                blob_hashes: vec![],
            }),
        )
        .await
        .unwrap();

        teardown();
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491349_execution_only() {
        setup();

        test_stitching_execution_only(
            op_sepolia_16491249_16491349(),
            None,
            vec![],
            vec![],
            #[cfg(feature = "experimental")]
            vec![],
        )
        .await
        .unwrap();

        teardown();
    }

    #[cfg(feature = "experimental")]
    pub async fn test_stitching_partials(
        boot_info: BootInfo,
        precondition_validation_data: Option<ProposalPrecondition>,
    ) -> anyhow::Result<()> {
        // Capture monolithic per-block partials alongside the execution trace.
        let (_executions, captured) = test_derivation_with_partials(
            boot_info.clone(),
            precondition_validation_data.clone(),
            None,
            None,
            Vec::new(),
        )
        .await
        .context("test_derivation_with_partials")?;

        // Monolithic pass: one partial per block.
        test_stitching(
            boot_info.clone(),
            precondition_validation_data.clone(),
            vec![],
            None,
            false,
            vec![],
            vec![],
            captured.clone(),
        );

        // Fully fragmented pass: one partial per transaction.
        let split = split_collected_partials(captured, usize::MAX);
        test_stitching(
            boot_info,
            precondition_validation_data,
            vec![],
            None,
            false,
            vec![],
            vec![],
            split,
        );

        Ok(())
    }

    #[cfg(feature = "experimental")]
    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491349_stitched_boots() {
        setup();

        test_stitching_boots(
            op_sepolia_16491249_16491349(),
            Some(ProposalPrecondition {
                proposal_l2_head_number: 16491249,
                proposal_output_count: 1,
                output_block_span: 100,
                blob_hashes: vec![],
            }),
            vec![],
        )
        .await
        .unwrap();

        teardown();
    }

    #[cfg(feature = "experimental")]
    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491250_stitched_partials() {
        setup();

        test_stitching_partials(
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
        .await
        .unwrap();

        teardown();
    }

    #[cfg(feature = "experimental")]
    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491349_stitched_partials() {
        setup();

        test_stitching_partials(
            op_sepolia_16491249_16491349(),
            Some(ProposalPrecondition {
                proposal_l2_head_number: 16491249,
                proposal_output_count: 1,
                output_block_span: 100,
                blob_hashes: vec![],
            }),
        )
        .await
        .unwrap();

        teardown();
    }

    /// `precompute_pe_boots` must skip empty per-block `partial_executions`
    /// without producing a stitched-boot entry (covers the
    /// `if block_partials.is_empty() { continue; }` guard).
    #[cfg(feature = "experimental")]
    #[test]
    fn precompute_pe_boots_skips_empty_block_partials() {
        let empty: Vec<Vec<PartialExecution>> = vec![vec![], vec![], vec![]];
        let result = precompute_pe_boots(&empty);
        assert!(result.is_empty());
    }

    /// Exercises the partial-execution short-circuit in
    /// `run_stitching_client`: when the recovered boot has the
    /// `0xFF…FF` sentinel `l1_head`, the function must build a
    /// `ProofJournal` directly from the boot+precondition and return
    /// without entering any stitching logic.
    #[cfg(feature = "experimental")]
    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_partial_exec_stitching_short_circuit() {
        // Capture a real partial via the existing derivation harness.
        let (executions, captured) = test_derivation_with_partials(
            op_sepolia_16491249_16491349(),
            None,
            None,
            Some(Default::default()),
            Vec::new(),
        )
        .await
        .unwrap();

        // Pick the first per-block partial and build a witness from it.
        let (partial, execution) = captured
            .iter()
            .zip(executions.iter())
            .find_map(|(partials, e)| partials.first().map(|p| (p, e)))
            .expect("fixture must include at least one partial");
        let witness = PartialExecutionWitness::from_preflight(partial.clone(), execution);

        // Build the matching PE boot (l1_head = 0xFF…FF) and oracle.
        let boot_info = make_pe_boot(&op_sepolia_16491249_16491349(), &witness);
        let oracle = Arc::new(TestOracle::new(boot_info.clone()));

        let (out_boot, journal, precondition) = KonaStitchingClient(EthereumDataSourceProvider)
            .run_stitching_client(
                B256::ZERO,
                oracle.clone(),
                oracle.clone(),
                OracleBlobProvider::new(oracle.clone()),
                B256::ZERO,
                Address::ZERO,
                vec![],
                None,
                false,
                vec![],
                vec![],
                Some(witness),
                Vec::new(),
            );

        // Boot is unchanged through the short-circuit and journal mirrors it.
        assert_eq!(out_boot.l1_head, L1_HEAD_TXN_ONLY_SENTINEL);
        assert_eq!(journal.l1_head, boot_info.l1_head);
        assert_eq!(
            journal.agreed_l2_output_root,
            boot_info.agreed_l2_output_root
        );
        assert_eq!(
            journal.claimed_l2_output_root,
            boot_info.claimed_l2_output_root
        );
        assert_eq!(
            journal.claimed_l2_block_number,
            boot_info.claimed_l2_block_number
        );
        // Precondition digest in the journal must match the precondition.
        assert_eq!(
            journal.precondition_hash,
            B256::new(precondition.digest().into())
        );
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
    #[should_panic(expected = "non-derivation proofs")]
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
}

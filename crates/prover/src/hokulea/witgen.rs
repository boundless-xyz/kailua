use crate::client::witgen;
use crate::client::witgen::OracleWitnessProvider;
use alloy_primitives::{Address, B256};
use canoe_verifier_address_fetcher::CanoeVerifierAddressFetcherDeployedByEigenLabs;
use hokulea_zkvm_verification::eigenda_witness_to_preloaded_provider;
use kailua_hokulea::canoe::KailuaCanoeVerifier;
use kailua_kona::boot::StitchedBootInfo;
use kailua_kona::driver::CachedDriver;
use kailua_kona::executor::Execution;
use kailua_kona::journal::ProofJournal;
use kailua_kona::oracle::local::LocalOnceOracle;
use kailua_kona::oracle::WitnessOracle;
use kailua_kona::precondition::Precondition;
use kailua_kona::witness::Witness;
use kona_derive::BlobProvider;
use kona_preimage::CommsClient;
use kona_proof::{BootInfo, FlushableCache};
use std::fmt::Debug;
use std::ops::DerefMut;
use std::sync::{Arc, Mutex};

#[allow(clippy::too_many_arguments)]
pub async fn run_hokulea_witgen_client<P, B, O>(
    preimage_oracle: Arc<P>,
    preimage_oracle_shard_size: usize,
    blob_provider: B,
    payout_recipient: Address,
    precondition_validation_data_hash: B256,
    execution_cache: Vec<Arc<Execution>>,
    derivation_cache: Option<CachedDriver>,
    trace_derivation: bool,
    stitched_preconditions: Vec<Precondition>,
    stitched_boot_info: Vec<StitchedBootInfo>,
) -> anyhow::Result<(
    BootInfo,
    ProofJournal,
    Precondition,
    Option<CachedDriver>,
    Witness<O>,
    hokulea_proof::eigenda_witness::EigenDAPreimage,
    O,
)>
where
    P: CommsClient + FlushableCache + Send + Sync + Debug + Clone + 'static,
    B: BlobProvider + Send + Sync + Debug + Clone,
    <B as BlobProvider>::Error: Debug,
    O: WitnessOracle + Send + Sync + Debug + Clone + Default + 'static,
{
    // Create witness target
    let eigen_witness = Arc::new(Mutex::new(Default::default()));
    let eigen_witness_aux = Arc::new(Mutex::new(O::default()));
    let eigen_witness_aux_oracle = Arc::new(OracleWitnessProvider {
        oracle: preimage_oracle.clone(),
        witness: eigen_witness_aux.clone(),
    });
    let eigen_oracle = Arc::new(LocalOnceOracle::new(eigen_witness_aux_oracle));
    // Create provider around witness
    let eigen = kailua_hokulea::da::EigenDADataSourceProvider(
        hokulea_witgen::witness_provider::OracleEigenDAPreimageProviderWithPreimage {
            provider: hokulea_proof::eigenda_provider::OracleEigenDAPreimageProvider::new(
                eigen_oracle.clone(),
            ),
            preimage: eigen_witness.clone(),
        },
    );
    // Instantiate verifier to populate data
    let (eigen_verifier, _) = KailuaCanoeVerifier::new(eigen_oracle.clone());
    eigenda_witness_to_preloaded_provider(
        eigen_oracle,
        eigen_verifier,
        CanoeVerifierAddressFetcherDeployedByEigenLabs {},
        Default::default(),
    )
    .await
    .expect("Failed to validate EigenDA Witness.");
    // Run regular witgen client
    let (boot, proof_journal, precondition, cached_driver, witness) =
        witgen::run_witgen_client::<P, B, O, _>(
            B256::from(bytemuck::cast::<_, [u8; 32]>(
                kailua_build::KAILUA_FPVM_HOKULEA_ID,
            )),
            preimage_oracle.clone(),
            preimage_oracle_shard_size,
            blob_provider,
            eigen,
            payout_recipient,
            precondition_validation_data_hash,
            execution_cache,
            derivation_cache,
            trace_derivation,
            stitched_preconditions,
            stitched_boot_info,
        )
        .await?;
    // Finalize witness
    let mut eigen_witness_aux = core::mem::take(eigen_witness_aux.lock().unwrap().deref_mut());
    // todo: shard eigen witness
    eigen_witness_aux.finalize_preimages(usize::MAX, true);

    // Return extended result
    let mut eigen_witness = eigen_witness.lock().unwrap();
    Ok((
        boot,
        proof_journal,
        precondition,
        cached_driver,
        witness,
        core::mem::take(eigen_witness.deref_mut()),
        eigen_witness_aux,
    ))
}

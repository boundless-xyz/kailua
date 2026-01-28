// Copyright 2025 RISC Zero, Inc.
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

use crate::canoe::KailuaCanoeVerifier;
use crate::da::EigenDADataSourceProvider;
use alloy_primitives::aliases::B256;
use alloy_primitives::Address;
use canoe_verifier_address_fetcher::CanoeVerifierAddressFetcherDeployedByEigenLabs;
use hokulea_proof::eigenda_witness::EigenDAWitness;
use hokulea_zkvm_verification::eigenda_witness_to_preloaded_provider;
use kailua_kona::boot::StitchedBootInfo;
use kailua_kona::client::stitching::{KonaStitchingClient, StitchingClient};
use kailua_kona::driver::CachedDriver;
use kailua_kona::executor::Execution;
use kailua_kona::journal::ProofJournal;
use kailua_kona::oracle::local::LocalOnceOracle;
use kailua_kona::precondition::Precondition;
use kona_derive::BlobProvider;
use kona_preimage::CommsClient;
use kona_proof::boot::BootInfo;
use kona_proof::FlushableCache;
use std::fmt::Debug;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct HokuleaStitchingClient<T: CommsClient + FlushableCache + Clone> {
    pub eigen_da_witness: EigenDAWitness,
    pub eigen_da_oracle: Arc<T>,
}

impl<T: CommsClient + FlushableCache + Clone> HokuleaStitchingClient<T> {
    pub fn new(eigen_da_witness: EigenDAWitness, eigen_da_oracle: Arc<T>) -> Self {
        Self {
            eigen_da_witness,
            eigen_da_oracle,
        }
    }
}

impl<
        O: CommsClient + FlushableCache + Send + Sync + Debug + 'static,
        B: BlobProvider + Send + Sync + Debug + Clone,
        T: CommsClient + FlushableCache + Send + Sync + Debug + 'static,
    > StitchingClient<O, B> for HokuleaStitchingClient<T>
{
    fn run_stitching_client(
        self,
        precondition_validation_data_hash: B256,
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
    ) -> (BootInfo, ProofJournal, Precondition)
    where
        <B as BlobProvider>::Error: Debug,
    {
        // Boot up eigenda verifier
        let eigen_oracle = Arc::new(LocalOnceOracle::new(self.eigen_da_oracle.clone()));
        let (eigen_verifier, boot) = KailuaCanoeVerifier::new(eigen_oracle.clone());

        // Run the stitching client with the EigenDA DASProvider
        let eigen_stitching_client = KonaStitchingClient(EigenDADataSourceProvider(
            kona_proof::block_on(eigenda_witness_to_preloaded_provider(
                eigen_oracle,
                eigen_verifier,
                CanoeVerifierAddressFetcherDeployedByEigenLabs {},
                self.eigen_da_witness,
            ))
            .expect("Failed to validate EigenDA Witness."),
        ));
        let (kona_boot_info, proof_journal, precondition) = eigen_stitching_client
            .run_stitching_client(
                precondition_validation_data_hash,
                oracle,
                stream,
                beacon,
                fpvm_image_id,
                payout_recipient_address,
                stitched_executions,
                derivation_cache,
                derivation_trace,
                stitched_preconditions,
                stitched_boot_info,
            );
        // Ensure boot record is the same for both oracles
        assert_eq!(boot, kona_boot_info);

        (kona_boot_info, proof_journal, precondition)
    }
}

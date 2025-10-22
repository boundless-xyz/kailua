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

use crate::client::witgen;
use crate::client::witgen::OracleWitnessProvider;
use alloy_primitives::{Address, B256};
use kailua_hana::da::CelestiaDataSourceProvider;
use kailua_hana::provider::HanaProvider;
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
pub async fn run_hana_witgen_client<P, B, O>(
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
    O,
)>
where
    P: CommsClient + FlushableCache + Send + Sync + Debug + Clone,
    B: BlobProvider + Send + Sync + Debug + Clone,
    <B as BlobProvider>::Error: Debug,
    O: WitnessOracle + Send + Sync + Debug + Clone + Default,
{
    // Create witness target
    let celestia_witness = Arc::new(Mutex::new(O::default()));
    let celestia_witness_oracle = Arc::new(OracleWitnessProvider {
        oracle: preimage_oracle.clone(),
        witness: celestia_witness.clone(),
    });
    let celestia_oracle = Arc::new(LocalOnceOracle::new(celestia_witness_oracle));
    // Create provider around witness
    let celestia = CelestiaDataSourceProvider(HanaProvider::new(celestia_oracle).0);
    // Run regular witgen client
    let (boot, proof_journal, precondition, cached_driver, witness) = witgen::run_witgen_client(
        B256::from(bytemuck::cast::<_, [u8; 32]>(
            kailua_build::KAILUA_FPVM_HANA_ID,
        )),
        preimage_oracle,
        preimage_oracle_shard_size,
        blob_provider,
        celestia,
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
    let mut celestia_witness = core::mem::take(celestia_witness.lock().unwrap().deref_mut());
    // todo: shard celestia witness
    celestia_witness.finalize_preimages(usize::MAX, true);
    // Return extended result
    Ok((
        boot,
        proof_journal,
        precondition,
        cached_driver,
        witness,
        celestia_witness,
    ))
}

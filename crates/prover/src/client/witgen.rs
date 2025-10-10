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

use alloy::consensus::Blob;
use alloy::eips::eip4844::IndexedBlobHash;
use alloy_primitives::{Address, B256};
use anyhow::Context;
use async_trait::async_trait;
use kailua_kona::blobs::BlobWitnessData;
use kailua_kona::boot::StitchedBootInfo;
use kailua_kona::client::core::{recover_collected_executions, DASourceProvider};
use kailua_kona::client::stitching::stitch_boot_info;
use kailua_kona::driver::CachedDriver;
use kailua_kona::executor::Execution;
use kailua_kona::journal::ProofJournal;
use kailua_kona::kona::OracleL1ChainProvider;
use kailua_kona::oracle::WitnessOracle;
use kailua_kona::precondition::Precondition;
use kailua_kona::witness::Witness;
use kona_derive::BlobProvider;
use kona_preimage::errors::PreimageOracleResult;
use kona_preimage::{CommsClient, HintWriterClient, PreimageKey, PreimageOracleClient};
use kona_proof::{BootInfo, FlushableCache};
use kona_protocol::BlockInfo;
use std::fmt::Debug;
use std::ops::DerefMut;
use std::sync::{Arc, Mutex};
use tracing::info;
use tracing::log::error;

#[allow(clippy::too_many_arguments)]
pub async fn run_witgen_client<P, B, O, D>(
    fpvm_image_id: B256,
    preimage_oracle: Arc<P>,
    preimage_oracle_shard_size: usize,
    blob_provider: B,
    da_source_provider: D,
    payout_recipient: Address,
    proposal_data_hash: B256,
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
)>
where
    P: CommsClient + FlushableCache + Send + Sync + Debug + Clone,
    B: BlobProvider + Send + Sync + Debug + Clone,
    <B as BlobProvider>::Error: Debug,
    O: WitnessOracle + Send + Sync + Debug + Clone + Default,
    D: DASourceProvider<OracleL1ChainProvider<OracleWitnessProvider<P, O>>, BlobWitnessProvider<B>>,
{
    let oracle_witness = Arc::new(Mutex::new(O::default()));
    let stream_witness = Arc::new(Mutex::new(O::default()));
    let blobs_witness = Arc::new(Mutex::new(BlobWitnessData::default()));
    info!("Preamble");
    let oracle = Arc::new(OracleWitnessProvider {
        oracle: preimage_oracle.clone(),
        witness: oracle_witness.clone(),
    });
    let stream = Arc::new(OracleWitnessProvider {
        oracle: preimage_oracle.clone(),
        witness: stream_witness.clone(),
    });
    let beacon = BlobWitnessProvider {
        provider: blob_provider,
        witness: blobs_witness.clone(),
    };

    // Run client
    let execution_trace = Arc::new(Mutex::new(Vec::new()));
    let derivation_trace = Arc::new(Mutex::new(None));
    let (boot, precondition) = kailua_kona::client::core::run_core_client(
        proposal_data_hash,
        oracle,
        stream.clone(),
        beacon,
        da_source_provider,
        execution_cache,
        Some(execution_trace.clone()),
        derivation_cache.clone(),
        trace_derivation.then(|| derivation_trace.clone()),
    )?;
    // Fix claimed output of captured executions
    let stitched_executions =
        recover_collected_executions(execution_trace, boot.claimed_l2_output_root);
    // Capture derivation snapshot
    let cached_driver = match derivation_trace.lock() {
        Ok(mut guard) => guard.take(),
        Err(err) => {
            error!("Failed to recover derivation driver snapshot: {err:?}");
            None
        }
    };
    // Stitch boot infos
    let (boot, journal_output, precondition) = stitch_boot_info(
        Some(stream),
        boot,
        fpvm_image_id,
        payout_recipient,
        precondition,
        stitched_preconditions.clone(),
        stitched_boot_info.clone(),
    )
    .await
    .context("Failed to stitch boot info")?;
    // Construct witness
    let mut witness = Witness {
        oracle_witness: core::mem::take(oracle_witness.lock().unwrap().deref_mut()),
        stream_witness: core::mem::take(stream_witness.lock().unwrap().deref_mut()),
        blobs_witness: core::mem::take(blobs_witness.lock().unwrap().deref_mut()),
        payout_recipient_address: payout_recipient,
        precondition_validation_data_hash: proposal_data_hash,
        stitched_executions: vec![stitched_executions],
        derivation_cache,
        trace_derivation,
        stitched_preconditions,
        stitched_boot_info,
        fpvm_image_id,
    };
    witness
        .oracle_witness
        .finalize_preimages(preimage_oracle_shard_size, true);
    witness
        .stream_witness
        .finalize_preimages(preimage_oracle_shard_size, false);
    // Return results
    Ok((boot, journal_output, precondition, cached_driver, witness))
}

#[derive(Clone, Debug)]
pub struct BlobWitnessProvider<T: BlobProvider> {
    pub provider: T,
    pub witness: Arc<Mutex<BlobWitnessData>>,
}

#[async_trait]
impl<T: BlobProvider + Send> BlobProvider for BlobWitnessProvider<T> {
    type Error = T::Error;

    async fn get_and_validate_blobs(
        &mut self,
        block_ref: &BlockInfo,
        blob_hashes: &[IndexedBlobHash],
    ) -> Result<Vec<Box<Blob>>, Self::Error> {
        let blobs = self
            .provider
            .get_and_validate_blobs(block_ref, blob_hashes)
            .await?;
        let settings = alloy::consensus::EnvKzgSettings::default();
        for blob in &blobs {
            let c_kzg_blob = c_kzg::Blob::from_bytes(blob.as_slice()).unwrap();
            let commitment = settings
                .get()
                .blob_to_kzg_commitment(&c_kzg_blob)
                .expect("Failed to convert blob to commitment");
            let proof = settings
                .get()
                .compute_blob_kzg_proof(&c_kzg_blob, &commitment.to_bytes())
                .unwrap();
            let mut witness = self.witness.lock().unwrap();
            witness.blobs.push(Blob::from(*c_kzg_blob));
            witness.commitments.push(commitment.to_bytes());
            witness.proofs.push(proof.to_bytes());
        }
        Ok(blobs)
    }
}

#[derive(Clone, Debug)]
pub struct OracleWitnessProvider<
    P: CommsClient + FlushableCache + Send + Sync + Debug + Clone,
    O: WitnessOracle,
> {
    pub oracle: Arc<P>,
    pub witness: Arc<Mutex<O>>,
}

impl<P, O> OracleWitnessProvider<P, O>
where
    P: CommsClient + FlushableCache + Send + Sync + Debug + Clone,
    O: WitnessOracle,
{
    pub fn save(&self, key: PreimageKey, value: &[u8]) {
        self.witness
            .lock()
            .unwrap()
            .insert_preimage(key, value.to_vec());
    }
}

#[async_trait]
impl<P, O> PreimageOracleClient for OracleWitnessProvider<P, O>
where
    P: CommsClient + FlushableCache + Send + Sync + Debug + Clone,
    O: WitnessOracle,
{
    async fn get(&self, key: PreimageKey) -> PreimageOracleResult<Vec<u8>> {
        match self.oracle.get(key).await {
            Ok(value) => {
                self.save(key, &value);
                Ok(value)
            }
            Err(e) => {
                error!(
                    "OracleWitnessProvider failed to get value for key {:?}/{}: {:?}",
                    key.key_type(),
                    key.key_value(),
                    e
                );
                Err(e)
            }
        }
    }

    async fn get_exact(&self, key: PreimageKey, buf: &mut [u8]) -> PreimageOracleResult<()> {
        match self.oracle.get_exact(key, buf).await {
            Ok(_) => {
                self.save(key, buf);
                Ok(())
            }
            Err(e) => {
                error!(
                    "OracleWitnessProvider failed to get exact value for key {:?}/{}: {:?}",
                    key.key_type(),
                    key.key_value(),
                    e
                );
                Err(e)
            }
        }
    }
}

#[async_trait]
impl<P, O> HintWriterClient for OracleWitnessProvider<P, O>
where
    P: CommsClient + FlushableCache + Send + Sync + Debug + Clone,
    O: WitnessOracle,
{
    async fn write(&self, hint: &str) -> PreimageOracleResult<()> {
        self.oracle.write(hint).await
    }
}

impl<P, O> FlushableCache for OracleWitnessProvider<P, O>
where
    P: CommsClient + FlushableCache + Send + Sync + Debug + Clone,
    O: WitnessOracle,
{
    fn flush(&self) {
        self.oracle.flush();
    }
}

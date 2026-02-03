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

use kailua_hokulea::stitching::HokuleaStitchingClient;
use kailua_kona::client::stateless::run_stateless_client;
use kailua_kona::oracle::vec::VecOracle;
use kailua_kona::oracle::WitnessOracle;
use kailua_kona::{client::log, witness::Witness};
use risc0_zkvm::guest::env;
use rkyv::rancor::Error;
use std::sync::Arc;

fn main() {
    // Load EigenDA blob witness
    let eigen_da = {
        let data = env::read_frame();
        log("DESERIALIZE EIGENDA CERTS");
        rkyv::from_bytes::<hokulea_proof::eigenda_witness::EigenDAWitness, Error>(&data)
            .expect("Failed to deserialize EigenDABlobWitnessData")
    };

    // Load EigenDA DA witness
    let eigen_da_aux = {
        // Read serialized witness data
        let witness_data = env::read_frame();
        log("DESERIALIZE EIGENDA ORACLE");
        rkyv::from_bytes::<VecOracle, Error>(&witness_data)
            .expect("Failed to deserialize eigen oracle witness")
    };
    eigen_da_aux
        .validate_preimages()
        .expect("Failed to validate eigen oracle preimages");

    // Load main witness
    let witness = {
        // Read serialized witness data
        let witness_data = env::read_frame();
        log("DESERIALIZE WITNESS");
        rkyv::from_bytes::<Witness<VecOracle>, Error>(&witness_data)
            .expect("Failed to deserialize witness")
    };

    // Load extension shards
    for (i, entry) in witness
        .oracle_witness
        .preimages
        .lock()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        if !entry.is_empty() {
            continue;
        }
        log(&format!("DESERIALIZE SHARD {i}"));
        // read_shard is undefined on non-zkvm platforms
        #[cfg(target_os = "zkvm")]
        let _ = core::mem::replace(entry, kailua_kona::oracle::vec::read_shard());
    }

    // Run client using witness data
    let proof_journal = run_stateless_client(
        witness,
        HokuleaStitchingClient::new(eigen_da, Arc::new(eigen_da_aux)),
    );

    // Write the final stitched journal
    env::commit_slice(&proof_journal.encode_packed());
}

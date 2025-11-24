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

use canoe_bindings::StatusCode;
use canoe_provider::CertVerifierCall;
use canoe_verifier::{CanoeVerifier, CertValidity, HokuleaCanoeVerificationError};
use eigenda_cert::AltDACommitment;
use kona_preimage::CommsClient;
use kona_proof::BootInfo;
use risc0_steel::ethereum::{
    EthChainSpec, EthEvmInput, ETH_HOLESKY_CHAIN_SPEC, ETH_MAINNET_CHAIN_SPEC,
    ETH_SEPOLIA_CHAIN_SPEC,
};
use risc0_steel::Contract;
use std::sync::Arc;

#[derive(Clone)]
pub struct KailuaCanoeVerifier<T: CommsClient + Send + Sync + 'static> {
    pub oracle: Arc<T>,
}

impl<T: CommsClient + Send + Sync + 'static> KailuaCanoeVerifier<T> {
    pub fn new(oracle: Arc<T>) -> (Self, BootInfo) {
        let boot = kona_proof::block_on(BootInfo::load(oracle.as_ref()))
            .expect("Failed to load boot info");
        (KailuaCanoeVerifier { oracle }, boot)
    }
}

impl<T: CommsClient + Send + Sync + 'static> CanoeVerifier for KailuaCanoeVerifier<T> {
    fn validate_cert_receipt(
        &self,
        cert_validity_pairs: Vec<(AltDACommitment, CertValidity)>,
        canoe_proof: Option<Vec<u8>>,
    ) -> Result<(), HokuleaCanoeVerificationError> {
        // Early abort with nothing to validate
        if cert_validity_pairs.is_empty() {
            return Ok(());
        }
        // Otherwise require proof data
        let Some(proof) = canoe_proof else {
            return Err(HokuleaCanoeVerificationError::MissingProof);
        };
        // Decode proof data into STEEL proof
        let evm_input: EthEvmInput = bincode::deserialize(&proof).map_err(|e| {
            HokuleaCanoeVerificationError::UnableToDeserializeReceipt(e.to_string())
        })?;
        // Load up boot information from oracle
        let boot = kona_proof::block_on(BootInfo::load(self.oracle.as_ref()))
            .expect("Failed to load boot info");
        let env = match boot.rollup_config.l1_chain_id {
            1 => evm_input.into_env(&ETH_MAINNET_CHAIN_SPEC),
            11155111 => evm_input.into_env(&ETH_SEPOLIA_CHAIN_SPEC),
            17000 => evm_input.into_env(&ETH_HOLESKY_CHAIN_SPEC),
            _ => evm_input.into_env(&EthChainSpec::new_single(
                boot.rollup_config.l1_chain_id,
                Default::default(),
            )),
        };
        // Validate each steel proof
        for (altda_commitment, cert_validity) in cert_validity_pairs {
            // Verify L1 chain data
            assert_eq!(boot.rollup_config.l1_chain_id, cert_validity.l1_chain_id);
            assert_eq!(boot.l1_head, cert_validity.l1_head_block_hash);
            // Verify certificate
            let is_valid = match CertVerifierCall::build(&altda_commitment) {
                CertVerifierCall::ABIEncodeInterface(call) => {
                    let status = Contract::new(cert_validity.verifier_address, &env)
                        .call_builder(&call)
                        .call();
                    status == StatusCode::SUCCESS as u8
                }
            };
            assert_eq!(is_valid, cert_validity.claimed_validity);
        }

        Ok(())
    }

    fn to_journals_bytes(&self, _: Vec<(AltDACommitment, CertValidity)>) -> Vec<u8> {
        // this method should not be used
        unimplemented!()
    }
}

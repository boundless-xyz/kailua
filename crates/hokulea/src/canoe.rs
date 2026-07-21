// Copyright 2025 Boundless Foundation, Inc.
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
use canoe_verifier_address_fetcher::{
    CanoeVerifierAddressFetcher, CanoeVerifierAddressFetcherDeployedByEigenLabs,
};
use eigenda_cert::AltDACommitment;
use kona_preimage::CommsClient;
use kona_proof::BootInfo;
use risc0_steel::Contract;
use risc0_steel::ethereum::{
    ETH_HOODI_CHAIN_SPEC, ETH_MAINNET_CHAIN_SPEC, ETH_SEPOLIA_CHAIN_SPEC, EthChainSpec, EthEvmInput,
};
use std::sync::Arc;

/// [CanoeVerifier] that validates EigenDA certificate validity claims inline with a Steel EVM
/// call, instead of verifying a separate Canoe proof through composition.
#[derive(Clone)]
pub struct KailuaCanoeVerifier<T: CommsClient + Send + Sync + 'static> {
    /// The oracle from which the boot record is loaded.
    pub oracle: Arc<T>,
}

impl<T: CommsClient + Send + Sync + 'static> KailuaCanoeVerifier<T> {
    /// Creates a verifier bound to `oracle` and returns the boot record loaded through it.
    /// Panics if the boot record cannot be loaded.
    pub fn new(oracle: Arc<T>) -> (Self, BootInfo) {
        let boot = kona_proof::block_on(BootInfo::load(oracle.as_ref()))
            .expect("Failed to load boot info");
        (KailuaCanoeVerifier { oracle }, boot)
    }
}

impl<T: CommsClient + Send + Sync + 'static> CanoeVerifier for KailuaCanoeVerifier<T> {
    /// Validates each claimed certificate validity by replaying the cert verifier contract call
    /// against the Steel EVM input carried in `canoe_proof`.
    ///
    /// Every claim must target the boot record's L1 chain and head, name the canonical
    /// EigenLabs-deployed cert verifier for that chain, and match the status code the verifier
    /// contract returns under Steel. Assertion failures abort the guest; an empty claim set
    /// requires no proof.
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
        // ensure STEEL proof uses l1 head as a reference
        let env = match boot.rollup_config.l1_chain_id {
            1 => evm_input.into_env(&ETH_MAINNET_CHAIN_SPEC),
            11155111 => evm_input.into_env(&ETH_SEPOLIA_CHAIN_SPEC),
            560048 => evm_input.into_env(&ETH_HOODI_CHAIN_SPEC),
            _ => evm_input.into_env(&EthChainSpec::new_single(
                boot.rollup_config.l1_chain_id,
                Default::default(),
            )),
        };
        assert_eq!(env.header().seal(), boot.l1_head);
        // Validate each steel proof
        let fetcher = CanoeVerifierAddressFetcherDeployedByEigenLabs {};
        for (altda_commitment, cert_validity) in cert_validity_pairs {
            // Verify L1 chain data
            assert_eq!(boot.rollup_config.l1_chain_id, cert_validity.l1_chain_id);
            assert_eq!(boot.l1_head, cert_validity.l1_head_block_hash);
            // Verify verifier address
            assert_eq!(
                cert_validity.verifier_address,
                fetcher
                    .fetch_address(
                        boot.rollup_config.l1_chain_id,
                        &altda_commitment.versioned_cert
                    )
                    .expect("Failed to fetch verifier address")
            );
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

    /// Unsupported: validation happens inline, so no Canoe journal is ever constructed.
    fn to_journals_bytes(&self, _: Vec<(AltDACommitment, CertValidity)>) -> Vec<u8> {
        unimplemented!()
    }
}

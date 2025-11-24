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

use crate::risczero::boundless::BoundlessArgs;
use alloy::providers::Provider;
use alloy::transports::http::reqwest::Url;
use alloy_primitives::B256;
use anyhow::{anyhow, bail, Context};
use async_trait::async_trait;
use canoe_bindings::StatusCode;
use canoe_provider::{CanoeInput, CanoeProvider, CertVerifierCall};
use kailua_sync::retry_res_timeout;
use risc0_steel::alloy::providers::ProviderBuilder;
use risc0_steel::ethereum::{
    EthChainSpec, EthEvmEnv, EthEvmInput, ETH_HOLESKY_CHAIN_SPEC, ETH_MAINNET_CHAIN_SPEC,
    ETH_SEPOLIA_CHAIN_SPEC,
};
use risc0_steel::host::BlockNumberOrTag;
use risc0_steel::Contract;
use std::str::FromStr;
use tracing::info;

/// A canoe provider implementation with steel
#[derive(Debug, Clone)]
pub struct KailuaCanoeSteelProvider {
    /// hash of l1 head block
    pub l1_head: B256,
    /// rpc to l1 geth node
    pub eth_rpc_url: String,
    /// Boundless arguments
    pub boundless_args: BoundlessArgs,
}

#[async_trait]
impl CanoeProvider for KailuaCanoeSteelProvider {
    type Receipt = EthEvmInput;

    async fn create_certs_validity_proof(
        &self,
        inputs: Vec<CanoeInput>,
    ) -> Option<anyhow::Result<Self::Receipt>> {
        // nothing to prove
        if inputs.is_empty() {
            return None;
        }
        // return result wrapped in Opt
        Some(self.prove(inputs).await)
    }
}

impl KailuaCanoeSteelProvider {
    async fn prove(&self, inputs: Vec<CanoeInput>) -> anyhow::Result<EthEvmInput> {
        // Instantiate L1
        let eth_rpc_url =
            Url::from_str(&self.eth_rpc_url).context("Failed to parse Ethereum RPC URL")?;

        // Create an alloy provider for that private key and URL.
        let l1_provider = ProviderBuilder::new().connect_http(eth_rpc_url);
        let l1_chain_id = retry_res_timeout!(15, l1_provider.get_chain_id().await).await;

        // Instantiate chain spec
        let chain_spec = match l1_chain_id {
            1 => ETH_MAINNET_CHAIN_SPEC.clone(),
            11155111 => ETH_SEPOLIA_CHAIN_SPEC.clone(),
            17000 => ETH_HOLESKY_CHAIN_SPEC.clone(),
            _ => EthChainSpec::new_single(l1_chain_id, Default::default()),
        };

        // Take the furthest l1 head as reference
        let l1_head_block = retry_res_timeout!(
            15,
            l1_provider
                .get_block_by_hash(self.l1_head)
                .await
                .context("get_block_by_hash")?
                .ok_or_else(|| anyhow!("Failed to fetch l1 head block"))
        )
        .await
        .number();

        info!("Begin to generate a Canoe proof using l1 block number {l1_head_block}");

        let mut env = EthEvmEnv::builder()
            .chain_spec(&chain_spec)
            .provider(l1_provider)
            .block_number_or_tag(BlockNumberOrTag::Number(l1_head_block))
            .build()
            .await?;

        // Prepare the function calls
        for input in &inputs {
            // Preflight the call to prepare the input that is required to execute the function in
            // the guest without RPC access. It also returns the result of the call.
            let mut contract = Contract::preflight(input.verifier_address, &mut env);

            let preflight_validity = match CertVerifierCall::build(&input.altda_commitment) {
                CertVerifierCall::ABIEncodeInterface(call) => {
                    let status = contract.call_builder(&call).call().await?;
                    status == StatusCode::SUCCESS as u8
                }
            };

            // Verify same outcome
            if input.claimed_validity != preflight_validity {
                bail!(
                    "claimed_validity={} != preflight_validity={}",
                    input.claimed_validity,
                    preflight_validity
                );
            }
        }

        // Construct the input from the environment.
        env.into_input().await
    }
}

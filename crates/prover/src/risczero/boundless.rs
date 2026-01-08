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

use crate::args::ProvingArgs;
use crate::client::proving::{acquire_owned_permit, SEMAPHORE_R0VM};
use crate::profiling::{Profile, ProfiledReceipt};
use crate::proof::save_to_bincoded_file;
use crate::proof::{proof_id, read_bincoded_file};
use crate::ProvingError;
use alloy::eips::BlockNumberOrTag;
use alloy::signers::k256::sha2::{Digest as _, Sha256};
use alloy::transports::http::reqwest::Url;
use alloy_primitives::{Address, B256, U256};
use anyhow::{anyhow, Context};
use aws_config::{BehaviorVersion, Region};
use aws_sdk_s3::config::Credentials;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client as S3Client;
use boundless_market::alloy::providers::Provider;
use boundless_market::alloy::signers::local::PrivateKeySigner;
use boundless_market::client::{Client, ClientError};
use boundless_market::contracts::boundless_market::MarketError;
use boundless_market::contracts::{
    Predicate, RequestError, RequestId, RequestStatus, Requirements,
};
use boundless_market::request_builder::{OfferParams, RequirementParams};
use boundless_market::storage::{StorageProviderConfig, StorageProviderType};
use boundless_market::{Deployment, GuestEnv, ProofRequest, StandardStorageProvider};
use bytemuck::NoUninit;
use clap::Parser;
use human_bytes::human_bytes;
use kailua_sync::{retry_res, retry_res_timeout};
use lazy_static::lazy_static;
use risc0_ethereum_contracts::selector::Selector;
use risc0_zkvm::sha::Digestible;
use risc0_zkvm::{default_executor, Digest, ExecutorEnv, Journal, Receipt};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::log::warn;
use tracing::{debug, error, info};

#[derive(Parser, Clone, Debug, Default)]
pub struct BoundlessArgs {
    /// Market provider for proof requests
    #[clap(flatten)]
    pub market: Option<MarketProviderConfig>,
    /// Storage provider for elf and input
    #[clap(flatten)]
    pub storage: Option<StorageProviderConfig>,

    /// Custom domain for file retrieval.
    /// Currently used to upload with a custom prefix and replace the download URL with this domain.
    #[clap(long, env)]
    pub r2_domain: Option<String>,
}

#[derive(Parser, Debug, Clone)]
#[group(requires_all = ["boundless_rpc_url", "boundless_wallet_key"])]
pub struct MarketProviderConfig {
    /// URL of the Ethereum RPC endpoint.
    #[clap(long, env, required = false)]
    pub boundless_rpc_url: Url,
    /// Private key used to interact with the EvenNumber contract.
    #[clap(long, env, required = false)]
    pub boundless_wallet_key: PrivateKeySigner,

    /// EIP-155 chain ID of the network hosting Boundless.
    ///
    /// This parameter takes precedent over all other deployment arguments if set to a known value
    #[clap(long, env, required = false)]
    pub boundless_chain_id: Option<u64>,
    /// Address of the [BoundlessMarket] contract.
    ///
    /// [BoundlessMarket]: crate::contracts::IBoundlessMarket
    #[clap(long, env, required = false)]
    pub boundless_market_address: Option<Address>,
    /// Address of the [RiscZeroVerifierRouter] contract.
    ///
    /// The verifier router implements [IRiscZeroVerifier]. Each network has a canonical router,
    /// that is deployed by the core team. You can additionally deploy and manage your own verifier
    /// instead. See the [Boundless docs for more details].
    ///
    /// [RiscZeroVerifierRouter]: https://github.com/risc0/risc0-ethereum/blob/main/contracts/src/RiscZeroVerifierRouter.sol
    /// [IRiscZeroVerifier]: https://github.com/risc0/risc0-ethereum/blob/main/contracts/src/IRiscZeroVerifier.sol
    /// [Boundless docs for more details]: https://docs.beboundless.xyz/developers/smart-contracts/verifier-contracts
    #[clap(
        long,
        env = "VERIFIER_ADDRESS",
        required = false,
        long_help = "Address of the RiscZeroVerifierRouter contract"
    )]
    pub boundless_verifier_router_address: Option<Address>,
    /// Address of the [RiscZeroSetVerifier] contract.
    ///
    /// [RiscZeroSetVerifier]: https://github.com/risc0/risc0-ethereum/blob/main/contracts/src/RiscZeroSetVerifier.sol
    #[clap(long, env, required = false)]
    pub boundless_set_verifier_address: Option<Address>,
    /// Address of the collateral token contract. The staking token is an ERC-20.
    #[clap(long, env, required = false)]
    pub boundless_collateral_token_address: Option<Address>,
    /// URL for the offchain [order stream service].
    ///
    /// [order stream service]: crate::order_stream_client
    #[clap(
        long,
        env,
        required = false,
        long_help = "URL for the offchain order stream service"
    )]
    pub boundless_order_stream_url: Option<Cow<'static, str>>,

    /// Whether to look back at prior proof requests
    #[clap(long, env, required = false, default_value_t = true)]
    pub boundless_look_back: bool,

    /// Whether to skip preflighting execution and assume a fixed cycle count.
    #[clap(long, env, required = false)]
    pub boundless_assume_cycle_count: Option<u64>,
    /// Starting price (wei) per cycle of the proving order
    #[clap(long, env, required = false, default_value = "200000000")]
    pub boundless_cycle_min_wei: U256,
    /// Maximum price (wei) per cycle of the proving order
    #[clap(long, env, required = false, default_value = "600000000")]
    pub boundless_cycle_max_wei: U256,
    /// Minimum megacycles per proving order
    #[clap(long, env, required = false, default_value_t = 250)]
    pub boundless_mega_cycle_min: u64,
    /// Collateral (ZKC) per megacycle of the proving order
    #[clap(long, env, required = false, default_value = "2500000000000000")]
    pub boundless_mega_cycle_collateral: U256,
    /// Minimum collateral (ZKC) per proving order
    #[clap(long, env, required = false, default_value = "5000000000000000000")]
    pub boundless_order_min_collateral: U256,
    /// Multiplier for delay before order price starts ramping up.
    #[clap(long, env, required = false, default_value_t = 0.5)]
    pub boundless_order_bid_delay_factor: f64,
    /// Minimum number of seconds to set as bid delay time
    #[clap(long, env, required = false, default_value_t = 120)]
    pub boundless_order_min_bid_delay: u64,
    /// Multiplier for order price to ramp up from min to max.
    #[clap(long, env, required = false, default_value_t = 1.0)]
    pub boundless_order_ramp_up_factor: f64,
    /// Minimum number of seconds to set as ramp up time
    #[clap(long, env, required = false, default_value_t = 600)]
    pub boundless_order_min_ramp_up: u32,
    /// Multiplier for order fulfillment timeout (seconds/segment) after locking
    #[clap(long, env, required = false, default_value_t = 3.0)]
    pub boundless_order_lock_timeout_factor: f64,
    /// Minimum number of seconds to set as lock timeout time
    #[clap(long, env, required = false, default_value_t = 1200)]
    pub boundless_order_min_lock_timeout: u32,
    /// Multiplier for order expiry timeout (seconds/segment) after lock timeout
    #[clap(long, env, required = false, default_value_t = 1.0)]
    pub boundless_order_expiry_factor: f64,
    /// Minimum number of seconds to set as expiry time
    #[clap(long, env, required = false, default_value_t = 900)]
    pub boundless_order_min_expiry: u32,
    /// Time in seconds between attempts to check order status
    #[clap(long, env, required = false, default_value_t = 12)]
    pub boundless_order_check_interval: u64,
    /// Whether to enable upload caching
    #[clap(long, env, required = false, default_value_t = true)]
    pub boundless_enable_upload_caching: bool,

    /// Time in seconds between attempts to submit new orders
    #[clap(long, env, required = false, default_value_t = 12)]
    pub boundless_order_submission_cooldown: u64,
}

impl MarketProviderConfig {
    pub fn to_arg_vec(
        &self,
        storage_provider_config: &Option<StorageProviderConfig>,
    ) -> Vec<String> {
        // RPC/Wallet args
        let mut proving_args = vec![
            String::from("--boundless-rpc-url"),
            self.boundless_rpc_url.to_string(),
            String::from("--boundless-wallet-key"),
            self.boundless_wallet_key.to_bytes().to_string(),
        ];
        // Boundless Deployment args
        if let Some(boundless_chain_id) = self.boundless_chain_id {
            proving_args.extend(vec![
                String::from("--boundless-chain-id"),
                boundless_chain_id.to_string(),
            ]);
        };
        if let Some(boundless_market_address) = &self.boundless_market_address {
            proving_args.extend(vec![
                String::from("--boundless-market-address"),
                boundless_market_address.to_string(),
            ]);
        };
        if let Some(boundless_verifier_router_address) = &self.boundless_verifier_router_address {
            proving_args.extend(vec![
                String::from("--boundless-verifier-router-address"),
                boundless_verifier_router_address.to_string(),
            ]);
        };
        if let Some(boundless_set_verifier_address) = &self.boundless_set_verifier_address {
            proving_args.extend(vec![
                String::from("--boundless-set-verifier-address"),
                boundless_set_verifier_address.to_string(),
            ]);
        };
        if let Some(boundless_collateral_token_address) = &self.boundless_collateral_token_address {
            proving_args.extend(vec![
                String::from("--boundless-collateral-token-address"),
                boundless_collateral_token_address.to_string(),
            ]);
        };
        if let Some(boundless_order_stream_url) = &self.boundless_order_stream_url {
            proving_args.extend(vec![
                String::from("--boundless-order-stream-url"),
                boundless_order_stream_url.to_string(),
            ]);
        };
        // Lookback
        if self.boundless_look_back {
            proving_args.push(String::from("--boundless-look-back"));
        }
        // Preflight skip
        if let Some(cycle_count) = self.boundless_assume_cycle_count {
            proving_args.extend(vec![
                String::from("--boundless-assume-cycle-count"),
                cycle_count.to_string(),
            ]);
        }
        // Proving fee args
        proving_args.extend(vec![
            String::from("--boundless-cycle-min-wei"),
            self.boundless_cycle_min_wei.to_string(),
            String::from("--boundless-cycle-max-wei"),
            self.boundless_cycle_max_wei.to_string(),
            String::from("--boundless-mega-cycle-min"),
            self.boundless_mega_cycle_min.to_string(),
            String::from("--boundless-mega-cycle-collateral"),
            self.boundless_mega_cycle_collateral.to_string(),
            String::from("--boundless-order-min-collateral"),
            self.boundless_order_min_collateral.to_string(),
            String::from("--boundless-order-bid-delay-factor"),
            self.boundless_order_bid_delay_factor.to_string(),
            String::from("--boundless-min-order-bid-delay"),
            self.boundless_order_min_bid_delay.to_string(),
            String::from("--boundless-order-ramp-up-factor"),
            self.boundless_order_ramp_up_factor.to_string(),
            String::from("--boundless-min-order-ramp-up"),
            self.boundless_order_min_ramp_up.to_string(),
            String::from("--boundless-order-lock-timeout-factor"),
            self.boundless_order_lock_timeout_factor.to_string(),
            String::from("--boundless-min-order-lock-timeout"),
            self.boundless_order_min_lock_timeout.to_string(),
            String::from("--boundless-order-expiry-factor"),
            self.boundless_order_expiry_factor.to_string(),
            String::from("--boundless-min-order-expiry"),
            self.boundless_order_min_expiry.to_string(),
            String::from("--boundless-order-check-interval"),
            self.boundless_order_check_interval.to_string(),
        ]);
        // Storage provider args
        if let Some(storage_cfg) = storage_provider_config {
            match &storage_cfg.storage_provider {
                StorageProviderType::S3 => {
                    proving_args.extend(vec![
                        String::from("--storage-provider"),
                        String::from("s3"),
                        String::from("--s3-access-key"),
                        storage_cfg.s3_access_key.clone().unwrap(),
                        String::from("--s3-secret-key"),
                        storage_cfg.s3_secret_key.clone().unwrap(),
                        String::from("--s3-bucket"),
                        storage_cfg.s3_bucket.clone().unwrap(),
                        String::from("--s3-url"),
                        storage_cfg.s3_url.clone().unwrap(),
                        String::from("--aws-region"),
                        storage_cfg.aws_region.clone().unwrap(),
                    ]);
                }
                StorageProviderType::Pinata => {
                    proving_args.extend(vec![
                        String::from("--storage-provider"),
                        String::from("pinata"),
                        String::from("--pinata-jwt"),
                        storage_cfg.pinata_jwt.clone().unwrap(),
                    ]);
                    if let Some(pinata_api_url) = &storage_cfg.pinata_api_url {
                        proving_args.extend(vec![
                            String::from("--pinata-api-url"),
                            pinata_api_url.to_string(),
                        ]);
                    }
                    if let Some(ipfs_gateway_url) = &storage_cfg.ipfs_gateway_url {
                        proving_args.extend(vec![
                            String::from("--ipfs-gateway-url"),
                            ipfs_gateway_url.to_string(),
                        ]);
                    }
                }
                StorageProviderType::File => {
                    proving_args.extend(vec![
                        String::from("--storage-provider"),
                        String::from("file"),
                    ]);
                    if let Some(file_path) = &storage_cfg.file_path {
                        proving_args.extend(vec![
                            String::from("--file-path"),
                            file_path.to_str().unwrap().to_string(),
                        ]);
                    }
                }
                _ => unimplemented!("Unknown storage provider."),
            }
        }
        proving_args
    }
}

lazy_static! {
    static ref BOUNDLESS_REQ: Arc<Mutex<()>> = Default::default();
    static ref BOUNDLESS_NET: Arc<Mutex<()>> = Default::default();
}

#[allow(clippy::too_many_arguments)]
pub async fn run_boundless_client<A: NoUninit + Into<Digest>>(
    market: MarketProviderConfig,
    storage: StorageProviderConfig,
    r2_domain: Option<String>,
    image: (A, &[u8]),
    journal: Journal,
    witness_slices: Vec<Vec<u32>>,
    witness_frames: Vec<Vec<u8>>,
    stitched_proofs: Vec<Receipt>,
    proving_args: &ProvingArgs,
    profile: Profile,
    data_dir: Option<&PathBuf>,
) -> Result<ProfiledReceipt, ProvingError> {
    info!("Running boundless client.");

    // Create R2 storage if configured
    let r2_storage = if let Some(ref domain) = r2_domain {
        Some(
            R2Storage::new(&storage, domain)
                .await
                .context("Failed to create R2 storage")
                .map_err(ProvingError::OtherError)?,
        )
    } else {
        None
    };

    // Instantiate storage provider (used when R2 is not configured)
    let storage_provider = StandardStorageProvider::from_config(&storage)
        .context("StandardStorageProvider::from_config")
        .map_err(ProvingError::OtherError)?;

    // Override deployment configuration if set
    let market_deployment = market
        .boundless_chain_id
        .and_then(Deployment::from_chain_id)
        .or_else(|| {
            let mut builder = Deployment::builder();
            if let Some(boundless_market_address) = market.boundless_market_address {
                builder.boundless_market_address(boundless_market_address);
            };
            if let Some(boundless_verifier_router_address) =
                market.boundless_verifier_router_address
            {
                builder.verifier_router_address(boundless_verifier_router_address);
            };
            if let Some(boundless_set_verifier_address) = market.boundless_set_verifier_address {
                builder.set_verifier_address(boundless_set_verifier_address);
            };
            if let Some(boundless_collateral_token_address) =
                market.boundless_collateral_token_address
            {
                builder.collateral_token_address(boundless_collateral_token_address);
            };
            if let Some(boundless_order_stream_url) = market.boundless_order_stream_url.clone() {
                builder.order_stream_url(boundless_order_stream_url);
            };
            builder.build().ok()
        });

    // Instantiate client
    let boundless_client = retry_res_timeout!(
        15,
        Client::builder()
            .with_private_key(market.boundless_wallet_key.clone())
            .with_rpc_url(market.boundless_rpc_url.clone())
            .with_deployment(market_deployment.clone())
            .with_storage_provider(Some(storage_provider.clone()))
            .build()
            .await
            .context("ClientBuilder::build()")
    )
    .await;

    // Report boundless deployment info
    info!(
        "Using BoundlessMarket at {}",
        boundless_client.deployment.boundless_market_address,
    );
    debug!("Deployment: {:?}", boundless_client.deployment);

    // Set the proof request requirements
    let requirements = Requirements::new(Predicate::digest_match(image.0, journal.digest()))
        // manually choose latest Groth16 receipt selector
        .with_selector((Selector::groth16_latest() as u32).into());

    // Wait for a market request to be fulfilled
    loop {
        // todo: price increase / timing relaxation
        match request_proof(
            &market,
            &boundless_client,
            r2_storage.as_ref(),
            image,
            journal.clone(),
            &witness_slices,
            &witness_frames,
            &stitched_proofs,
            proving_args,
            &requirements,
            profile.clone(),
            data_dir,
        )
        .await
        {
            Err(ProvingError::OtherError(e)) => {
                error!("(Retrying) Boundless request failed: {e:?}");
                sleep(Duration::from_secs(1)).await;
            }
            // this will return successful results or propagatable errors
            result => break result,
        }
    }
}

pub fn next_nonce(requirements: &Requirements, previous_nonce: Option<u32>) -> u32 {
    let pred_type = (requirements.predicate.predicateType as u128).to_be_bytes();
    let prev_nonce = previous_nonce.unwrap_or(u32::MAX).to_be_bytes();
    let req_params = RequirementParams::try_from(requirements.clone()).unwrap();
    let data = [
        requirements.selector.as_slice(),
        req_params.image_id.unwrap_or_default().as_slice(),
        pred_type.as_slice(),
        requirements.callback.addr.as_slice(),
        requirements.callback.gasLimit.as_le_slice(),
        prev_nonce.as_slice(),
        requirements.predicate.data.as_ref(),
    ]
    .concat();
    let digest = data.digest().as_bytes().to_vec();
    u32::from_be_bytes(digest[..4].try_into().unwrap())
}

pub async fn get_proof_request(
    market: &MarketProviderConfig,
    boundless_client: &Client,
    request_id: U256,
) -> Option<ProofRequest> {
    loop {
        // Bypass order stream check if not specified in config
        match market.boundless_order_stream_url.is_some() {
            true => {
                match boundless_client
                    .fetch_proof_request(request_id, None, None)
                    .await
                {
                    Ok((req, _)) => break Some(req),
                    Err(err) => {
                        // No request for nonce
                        if matches!(
                            err,
                            ClientError::MarketError(MarketError::RequestNotFound(_))
                        ) {
                            break None;
                        }
                        // Some other error that needs us to retry
                        error!("fetch_proof_request error: {err:?}");
                        sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                }
            }
            false => {
                match boundless_client
                    .boundless_market
                    .get_submitted_request(request_id, None)
                    .await
                {
                    Ok((req, _)) => break Some(req),
                    Err(err) => {
                        // No request for nonce
                        if matches!(err, MarketError::RequestNotFound(_)) {
                            break None;
                        }
                        // Some other error that needs us to retry
                        error!("get_submitted_request error: {err:?}");
                        sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                }
            }
        }
    }
}

pub async fn get_next_fresh_nonce(
    market: &MarketProviderConfig,
    boundless_client: &Client,
    requirements: &Requirements,
    mut previous_nonce: Option<u32>,
) -> u32 {
    let boundless_wallet_address = boundless_client.signer.as_ref().unwrap().address();

    loop {
        let nonce = next_nonce(requirements, previous_nonce);
        let request_id = RequestId::u256(boundless_wallet_address, nonce);
        info!("Checking request freshness for {request_id:x}");
        // Return true if fresh request id
        if get_proof_request(market, boundless_client, request_id)
            .await
            .is_none()
        {
            break nonce;
        }
        // A request exists
        previous_nonce = Some(nonce);
    }
}

pub async fn look_back(
    market: &MarketProviderConfig,
    boundless_client: &Client,
    requirements: &Requirements,
    proving_args: &ProvingArgs,
    previous_nonce: &mut Option<u32>,
    profile: &Profile,
) -> Result<Option<ProfiledReceipt>, ProvingError> {
    let boundless_wallet_address = boundless_client.signer.as_ref().unwrap().address();
    loop {
        let nonce = next_nonce(requirements, *previous_nonce);
        let request_id = RequestId::u256(boundless_wallet_address, nonce);
        let _ = previous_nonce.insert(nonce);
        info!("Looking back at request {request_id:x}");
        // Get request behind id
        let Some(request) = get_proof_request(market, boundless_client, request_id).await else {
            // we hit a fresh nonce
            break Ok(None);
        };
        // Check if not expired
        let request_status = retry_res_timeout!(
            15,
            boundless_client
                .boundless_market
                .get_status(request_id, Some(request.expires_at()))
                .await
                .context("get_status")
        )
        .await;

        if matches!(request_status, RequestStatus::Expired) {
            // We found a duplicate but it was expired
            continue;
        }

        // Skip unrelated request with nonce collision
        if &request.requirements != requirements {
            continue;
        }

        info!("Found matching request already submitted!");

        if proving_args.skip_await_proof {
            warn!("Skipping awaiting proof on Boundless.");
            return Err(ProvingError::NotAwaitingProof);
        }

        // Return result if okay
        let req_params = RequirementParams::try_from(requirements.clone()).unwrap();
        match retrieve_proof(
            boundless_client,
            request_id,
            req_params.image_id.unwrap_or_default().0,
            market.boundless_order_check_interval,
            request.expires_at(),
            profile.clone(),
        )
        .await
        {
            Ok(proof) => {
                break Ok(Some(proof));
            }
            Err(err) => {
                error!("Failed to retrieve proof: {err:?}");
                // continue to next nonce
            }
        }
    }
}

pub async fn retrieve_proof(
    boundless_client: &Client,
    request_id: U256,
    image_id: impl Into<Digest>,
    interval: u64,
    expires_at: u64,
    mut profile: Profile,
) -> Result<ProfiledReceipt, ClientError> {
    // Wait for the request to be fulfilled by the market, returning the journal and seal.
    info!("Waiting for 0x{request_id:x} to be fulfilled");

    loop {
        match boundless_client
            .wait_for_request_fulfillment(request_id, Duration::from_secs(interval), expires_at)
            .await
        {
            Ok(fulfillment) => {
                let journal = fulfillment
                    .data()
                    .context("Failed to decode request Fulfillment data.")?
                    .journal()
                    .cloned()
                    .unwrap_or_default()
                    .to_vec();
                let Ok(risc0_ethereum_contracts::receipt::Receipt::Base(receipt)) =
                    risc0_ethereum_contracts::receipt::decode_seal(
                        fulfillment.seal,
                        image_id,
                        journal,
                    )
                else {
                    return Err(ClientError::RequestError(RequestError::MissingRequirements));
                };

                // Find proving cost
                let price_point = if retry_res_timeout!(
                    15,
                    boundless_client
                        .boundless_market
                        .is_locked(request_id)
                        .await
                )
                .await
                {
                    retry_res_timeout!(
                        15,
                        boundless_client
                            .boundless_market
                            .query_request_locked_event(request_id, None, None)
                            .await
                    )
                    .await
                    .block_number
                } else {
                    retry_res_timeout!(
                        15,
                        boundless_client
                            .boundless_market
                            .query_request_fulfilled_event(request_id, None, None)
                            .await
                    )
                    .await
                    .block_number
                };

                let timestamp = retry_res_timeout!(
                    15,
                    boundless_client
                        .provider()
                        .get_block_by_number(BlockNumberOrTag::Number(price_point))
                        .await
                        .context("get_block_by_number")?
                        .ok_or_else(|| anyhow!("Failed to fetch block"))
                )
                .await
                .header
                .timestamp;

                let (proof_request, _) = retry_res_timeout!(
                    15,
                    boundless_client
                        .fetch_proof_request(request_id, None, None)
                        .await
                        .context("fetch_proof_request")
                )
                .await;
                match proof_request.offer.price_at(timestamp) {
                    Ok(cost) => {
                        profile = profile.with_boundless_cost(cost);
                    }
                    Err(err) => {
                        error!("Could not calculate boundless request {request_id} price at {timestamp}: {err:?}");
                    }
                }

                return Ok((*receipt, profile));
            }
            Err(e) => {
                if matches!(
                    e,
                    ClientError::MarketError(MarketError::RequestHasExpired(_))
                ) {
                    return Err(e);
                }
                // Try again
                error!("Failed to wait for fulfillment: {e:?}");
                sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn request_proof<A: NoUninit + Into<Digest>>(
    market: &MarketProviderConfig,
    boundless_client: &Client,
    r2_storage: Option<&R2Storage>,
    image: (A, &[u8]),
    journal: Journal,
    witness_slices: &Vec<Vec<u32>>,
    witness_frames: &Vec<Vec<u8>>,
    stitched_proofs: &Vec<Receipt>,
    proving_args: &ProvingArgs,
    requirements: &Requirements,
    profile: Profile,
    data_dir: Option<&PathBuf>,
) -> Result<ProfiledReceipt, ProvingError> {
    // Get cycle count
    let req_file_name = request_file_name(image.0, journal.clone());
    let request_cycles = match (
        market.boundless_assume_cycle_count,
        read_bincoded_file::<BoundlessRequest>(data_dir, &req_file_name).await,
    ) {
        (_, Ok(request)) => {
            // we sleep here so to avoid pinata api rate limits
            sleep(Duration::from_secs(2)).await;
            request
        }
        (Some(cycle_count), _) => {
            // we sleep here so to avoid pinata api rate limits
            sleep(Duration::from_secs(2)).await;
            BoundlessRequest {
                total_cycle_count: cycle_count,
                user_cycle_count: cycle_count,
            }
        }
        (None, Err(err)) => {
            warn!("Preflighting execution: {err:?}");
            let preflight_witness_slices = witness_slices.clone();
            let preflight_witness_frames = witness_frames.clone();
            let preflight_stitched_proofs = stitched_proofs.clone();
            let segment_limit = proving_args.segment_limit;
            let elf = image.1.to_vec();
            let r0vm_permit = acquire_owned_permit(SEMAPHORE_R0VM.clone())
                .await
                .context("acquire_owned_permit")
                .map_err(ProvingError::OtherError)?;
            let session_info = tokio::task::spawn_blocking(move || {
                let mut builder = ExecutorEnv::builder();
                // Set segment po2
                builder.segment_limit_po2(segment_limit);
                // Pass in witness data slices
                for slice in &preflight_witness_slices {
                    builder.write_slice(slice);
                }
                // Pass in witness data frames
                for frame in &preflight_witness_frames {
                    builder.write_frame(frame);
                }
                // Pass in proofs
                for proof in &preflight_stitched_proofs {
                    builder.write(proof).context("env::write")?;
                }
                let env = builder.build().context("env::build")?;
                let session_info = default_executor()
                    .execute(env, &elf)
                    .context("Executor::execute")?;
                Ok::<_, anyhow::Error>(session_info)
            })
            .await
            .context("spawn_blocking")
            .map_err(ProvingError::OtherError)?
            .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
            drop(r0vm_permit);
            let total_cycle_count =
                (1u64 << segment_limit as u64) * (session_info.segments.len() as u64);
            let cached_data = BoundlessRequest {
                total_cycle_count,
                user_cycle_count: session_info.cycles(),
            };
            if let Err(err) = save_to_bincoded_file(&cached_data, data_dir, &req_file_name).await {
                warn!("Failed to cache cycle count data: {err:?}");
            }
            cached_data
        }
    };
    info!("Request cycle count: {}.", request_cycles.total_cycle_count);
    // Set profiled cycle count data
    let profile = profile.with_cycle_counts(
        request_cycles.system_cycle_count(),
        request_cycles.user_cycle_count,
    );

    // Check prior requests
    let fresh_nonce = if market.boundless_look_back {
        let mut nonce_target = None;
        // note: look_back only returns the NotAwaitingProof error
        if let Some(proof) = look_back(
            market,
            boundless_client,
            requirements,
            proving_args,
            &mut nonce_target,
            &profile,
        )
        .await?
        {
            return Ok(proof);
        }
        nonce_target.unwrap()
    } else {
        get_next_fresh_nonce(market, boundless_client, requirements, None).await
    };

    // Upload program
    let bin_file_name = binary_file_name(image.0);
    let program_url = loop {
        match (
            market.boundless_enable_upload_caching,
            read_bincoded_file::<String>(data_dir, &bin_file_name)
                .await
                .map(|s| Url::parse(&s)),
        ) {
            (true, Ok(Ok(url))) => {
                info!("Using Kailua binary previously uploaded to {url}.");
                break url;
            }
            _ => {
                // Only one prover may upload data at a time
                let Ok(boundless_net_lock) = BOUNDLESS_NET.try_lock() else {
                    sleep(Duration::from_secs(1)).await;
                    continue;
                };

                info!(
                    "Uploading {} Kailua ELF.",
                    human_bytes(image.1.len() as f64)
                );
                let program_url = if let Some(r2) = r2_storage {
                    retry_res!(r2
                        .upload_program(image.1)
                        .await
                        .context("R2Storage::upload_program"))
                    .await
                } else {
                    retry_res!(boundless_client
                        .upload_program(image.1)
                        .await
                        .context("Client::upload_program"))
                    .await
                };
                if let Err(err) =
                    save_to_bincoded_file(&program_url.to_string(), data_dir, &bin_file_name).await
                {
                    warn!("Failed to cache Kailua program url: {err:?}");
                }
                drop(boundless_net_lock);
                break program_url;
            }
        };
    };
    info!("Kailua ELF URL: {program_url}");

    // Upload input
    let inp_file_name = input_file_name(image.0, journal.clone());
    let input_url = loop {
        match (
            market.boundless_enable_upload_caching,
            read_bincoded_file::<String>(data_dir, &inp_file_name)
                .await
                .map(|s| Url::parse(&s)),
        ) {
            (true, Ok(Ok(url))) => {
                break url;
            }
            _ => {
                // Only one prover may upload data at a time
                let Ok(boundless_net_lock) = BOUNDLESS_NET.try_lock() else {
                    sleep(Duration::from_secs(1)).await;
                    continue;
                };

                let mut guest_env_builder = GuestEnv::builder();
                // Pass in input slices
                for slice in witness_slices {
                    guest_env_builder = guest_env_builder.write_slice(slice);
                }
                // Pass in input frames
                for frame in witness_frames {
                    guest_env_builder = guest_env_builder.write_frame(frame);
                }
                // Pass in proofs
                for proof in stitched_proofs {
                    guest_env_builder = guest_env_builder
                        .write(proof)
                        .context("GuestEnvBuilder::write")
                        .map_err(ProvingError::OtherError)?;
                }
                // Build input vector
                let input = guest_env_builder
                    .build_vec()
                    .context("GuestEnvBuilder::build_vec")
                    .map_err(ProvingError::OtherError)?;

                // Upload input
                info!("Uploading {} input data.", human_bytes(input.len() as f64));
                let input_url = if let Some(r2) = r2_storage {
                    retry_res!(r2
                        .upload_input(&input)
                        .await
                        .context("R2Storage::upload_input"))
                    .await
                } else {
                    retry_res!(boundless_client
                        .upload_input(&input)
                        .await
                        .context("Client::upload_input"))
                    .await
                };
                drop(boundless_net_lock);
                // avoid api rate limits
                sleep(Duration::from_secs(2)).await;
                if let Err(err) =
                    save_to_bincoded_file(&input_url.to_string(), data_dir, &inp_file_name).await
                {
                    warn!("Failed to cache Kailua input url: {err:?}");
                }
                break input_url;
            }
        }
    };
    info!("Input URL: {input_url}.");

    // Only one prover may submit a request at a time
    let boundless_req_lock = BOUNDLESS_REQ.lock().await;
    // Build final request
    let boundless_wallet_address = boundless_client.signer.as_ref().unwrap().address();

    let boundless_rpc_time = retry_res_timeout!(
        15,
        boundless_client
            .provider()
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await
            .context("get_block_by_number latest")?
            .ok_or_else(|| anyhow!("Failed to fetch latest block from Boundless RPC"))
    )
    .await
    .header
    .timestamp;

    let priced_cycles =
        U256::from((market.boundless_mega_cycle_min << 20).max(request_cycles.total_cycle_count));
    let min_price = market.boundless_cycle_min_wei * priced_cycles;
    let max_price = market.boundless_cycle_max_wei * priced_cycles;
    let timed_mega_cycles = market
        .boundless_mega_cycle_min
        .max(request_cycles.total_cycle_count.div_ceil(1 << 20)) as f64;
    let bid_delay_time = market
        .boundless_order_min_bid_delay
        .max((market.boundless_order_bid_delay_factor * timed_mega_cycles) as u64);
    let ramp_up_period = market
        .boundless_order_min_ramp_up
        .max((market.boundless_order_ramp_up_factor * timed_mega_cycles) as u32);
    let lock_timeout = ramp_up_period
        + market
            .boundless_order_min_lock_timeout
            .max((market.boundless_order_lock_timeout_factor * timed_mega_cycles) as u32);
    let expiry = lock_timeout
        + market
            .boundless_order_min_expiry
            .max((market.boundless_order_expiry_factor * timed_mega_cycles) as u32);
    let lock_collateral = market
        .boundless_order_min_collateral
        .max(market.boundless_mega_cycle_collateral * U256::from(timed_mega_cycles));
    let request = boundless_client
        .new_request()
        .with_journal(journal)
        .with_cycles(request_cycles.total_cycle_count)
        .with_program_url(program_url)
        .context("RequestParams::with_program_url")
        .map_err(ProvingError::OtherError)?
        .with_input_url(input_url)
        .context("RequestParams::with_input_url")
        .map_err(ProvingError::OtherError)?
        .with_requirements(
            RequirementParams::try_from(requirements.clone())
                .context("Failed to convert Requirements")
                .map_err(ProvingError::OtherError)?,
        )
        .with_offer(
            OfferParams::builder()
                .min_price(min_price)
                .max_price(max_price)
                .bidding_start(boundless_rpc_time + bid_delay_time)
                .lock_collateral(lock_collateral)
                .ramp_up_period(ramp_up_period)
                .lock_timeout(lock_timeout)
                .timeout(expiry)
                .build()
                .context("OfferParamsBuilder::build()")
                .map_err(ProvingError::OtherError)?,
        )
        .with_request_id(RequestId::new(boundless_wallet_address, fresh_nonce));

    // Send the request and wait for it to be completed.
    let (request_id, expires_at) = if market.boundless_order_stream_url.is_some() {
        info!("Submitting offchain request.");
        boundless_client
            .submit_offchain(request.clone())
            .await
            .context("Client::submit_offchain()")
            .map_err(ProvingError::OtherError)?
    } else {
        info!("Submitting onchain request.");
        boundless_client
            .submit_onchain(request.clone())
            .await
            .context("Client::submit_onchain()")
            .map_err(ProvingError::OtherError)?
    };
    info!(
        "Boundless request 0x{request_id:x} submitted. ({} sec cooldown).",
        market.boundless_order_submission_cooldown
    );
    sleep(Duration::from_secs(
        market.boundless_order_submission_cooldown,
    ))
    .await;
    drop(boundless_req_lock);

    if proving_args.skip_await_proof {
        warn!("Skipping awaiting proof on Boundless.");
        return Err(ProvingError::NotAwaitingProof);
    }

    // Wait for request fulfillment
    retrieve_proof(
        boundless_client,
        request_id,
        image.0,
        market.boundless_order_check_interval,
        expires_at,
        profile,
    )
    .await
    .context("retrieve_proof")
    .map_err(|e| ProvingError::OtherError(anyhow!(e)))
}

pub fn request_file_name<A: NoUninit>(image_id: A, journal: impl Into<Journal>) -> String {
    format!("boundless-{}.req", proof_id(image_id, journal))
}

pub fn binary_file_name<A: NoUninit>(image_id: A) -> String {
    format!(
        "boundless-{}.req",
        B256::from(bytemuck::cast::<A, [u8; 32]>(image_id))
    )
}

pub fn input_file_name<A: NoUninit>(image_id: A, journal: impl Into<Journal>) -> String {
    format!("boundless-{}.req", !proof_id(image_id, journal))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BoundlessRequest {
    /// Number of cycles that require proving
    pub total_cycle_count: u64,
    /// Number of user cycles
    pub user_cycle_count: u64,
}

impl BoundlessRequest {
    pub fn system_cycle_count(&self) -> u64 {
        self.total_cycle_count - self.user_cycle_count
    }
}

/// R2 Storage implementation for manual uploads with custom prefix
// TODO this is a workaround to handle our specific R2 usage. This logic encapsulates the ability to
// upload the file with a custom prefix (defaulting to v2/kailua) and also using a custom domain for
// retrieving the files. If neither of these are needed, the base S3 client will be sufficient.
pub struct R2Storage {
    client: S3Client,
    bucket: String,
    domain: String,
}

impl R2Storage {
    pub async fn new(
        storage_config: &StorageProviderConfig,
        r2_domain: &str,
    ) -> anyhow::Result<Self> {
        // Extract S3 configuration
        let access_key = storage_config
            .s3_access_key
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("S3 access key required for R2"))?;
        let secret_key = storage_config
            .s3_secret_key
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("S3 secret key required for R2"))?;
        let bucket = storage_config
            .s3_bucket
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("S3 bucket required for R2"))?;
        let endpoint = storage_config
            .s3_url
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("S3 URL (endpoint) required for R2"))?;
        let region = storage_config
            .aws_region
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("AWS region required for R2"))?;

        let credentials = Credentials::new(access_key, secret_key, None, None, "R2");

        let aws_config = aws_config::defaults(BehaviorVersion::latest())
            .credentials_provider(credentials)
            .endpoint_url(endpoint)
            .region(Region::new(region.clone()))
            .load()
            .await;

        let client = S3Client::new(&aws_config);

        let domain = if r2_domain.starts_with("http://") || r2_domain.starts_with("https://") {
            r2_domain.to_string()
        } else {
            format!("https://{r2_domain}")
        };

        Ok(Self {
            client,
            bucket: bucket.clone(),
            domain,
        })
    }

    async fn upload(&self, key: &str, data: Vec<u8>) -> anyhow::Result<Url> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(data))
            .send()
            .await
            .context("Failed to upload to R2")?;

        Ok(Url::parse(&format!("{}/{}", self.domain, key))?)
    }

    async fn upload_program(&self, program: &[u8]) -> anyhow::Result<Url> {
        let image_id = risc0_zkvm::compute_image_id(program)?;
        let key = format!("v2/kailua/program/{image_id}");
        self.upload(&key, program.to_vec()).await
    }

    async fn upload_input(&self, input: &[u8]) -> anyhow::Result<Url> {
        let digest = Sha256::digest(input);
        let key = format!("v2/kailua/input/{}.bin", hex::encode(digest.as_slice()));
        self.upload(&key, input.to_vec()).await
    }
}

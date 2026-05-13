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

#![cfg(feature = "devnet")]
#![recursion_limit = "256"]

use alloy::eips::{BlockId, BlockNumberOrTag};
use alloy::network::BlockResponse;
use alloy::providers::Provider;
use anyhow::{anyhow, Context};
use kailua_cli::fast_track::{fast_track, FastTrackArgs};
#[cfg(not(feature = "eigen"))]
use kailua_cli::fault::{fault, FaultArgs};
#[cfg(not(feature = "eigen"))]
use kailua_proposer::args::ProposeArgs;
#[cfg(not(feature = "eigen"))]
use kailua_proposer::propose::propose;
use kailua_prover::args::{ProveArgs, ProvingArgs};
use kailua_prover::prove::prove;
use kailua_sync::agent::SyncAgent;
use kailua_sync::args::SyncArgs;
use kailua_sync::provider::ProviderArgs;
use kailua_sync::transact::signer::{DeployerSignerArgs, GuardianSignerArgs, OwnerSignerArgs};
#[cfg(not(feature = "eigen"))]
use kailua_sync::transact::signer::{ProposerSignerArgs, ValidatorSignerArgs};
use kailua_sync::transact::TransactArgs;
#[cfg(not(feature = "eigen"))]
use kailua_validator::args::{PermitPolicy, ValidateArgs};
#[cfg(not(feature = "eigen"))]
use kailua_validator::validate::validate;
use lazy_static::lazy_static;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::env::set_var;
use std::fs;
#[cfg(feature = "eigen")]
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::tempdir;
use tokio::io;
#[cfg(feature = "eigen")]
use tokio::process::Child;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::sleep;
#[cfg(not(feature = "eigen"))]
use tokio::try_join;

lazy_static! {
    static ref DEVNET: Arc<Mutex<()>> = Default::default();
}

const WORKSPACE_ROOT: &str = env!("CARGO_MANIFEST_DIR");
const DEVNET_READINESS_TIMEOUT: Duration = Duration::from_secs(300);
const DEVNET_POLL_INTERVAL: Duration = Duration::from_secs(2);
const DEPLOYER_ALIAS: &str = "deployer";
const OWNER_ALIAS: &str = "owner";
const GUARDIAN_ALIAS: &str = "guardian";
#[cfg(not(feature = "eigen"))]
const PROPOSER_ALIAS: &str = "proposer";
#[cfg(not(feature = "eigen"))]
const VALIDATOR_ALIAS: &str = "validator";
#[cfg(not(feature = "eigen"))]
const FAULT_PROPOSER_ALIAS: &str = "fault-proposer";
#[cfg(not(feature = "eigen"))]
const TRAIL_FAULT_PROPOSER_ALIAS: &str = "trail-fault-proposer";
const VANGUARD_ALIAS: &str = "vanguard";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DevnetFlavor {
    Standard,
    EigenDA,
}

#[derive(Clone, Debug, Deserialize)]
struct DevnetConfig {
    l1: ChainDescriptor,
    l2: Vec<L2ChainDescriptor>,
    #[serde(default)]
    auxiliary_services: HashMap<String, ServiceDescriptor>,
}

#[derive(Clone, Debug, Deserialize)]
struct L2ChainDescriptor {
    #[serde(flatten)]
    chain: ChainDescriptor,
}

#[derive(Clone, Debug, Deserialize)]
struct ChainDescriptor {
    nodes: Vec<NodeDescriptor>,
    #[serde(default)]
    wallets: HashMap<String, WalletDescriptor>,
}

#[derive(Clone, Debug, Deserialize)]
struct NodeDescriptor {
    services: HashMap<String, ServiceDescriptor>,
}

#[derive(Clone, Debug, Deserialize)]
struct ServiceDescriptor {
    endpoints: HashMap<String, EndpointDescriptor>,
}

#[derive(Clone, Debug, Deserialize)]
struct EndpointDescriptor {
    host: String,
    port: u16,
    #[serde(default)]
    scheme: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct WalletDescriptor {
    address: String,
    private_key: String,
}

impl DevnetConfig {
    fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let descriptor = fs::read_to_string(path)
            .with_context(|| format!("Failed to read devnet descriptor at {}", path.display()))?;
        serde_json::from_str(&descriptor)
            .with_context(|| format!("Failed to parse devnet descriptor at {}", path.display()))
    }

    fn l2_chain(&self) -> anyhow::Result<&ChainDescriptor> {
        self.l2
            .first()
            .map(|chain| &chain.chain)
            .ok_or_else(|| anyhow!("Missing L2 chain in devnet descriptor"))
    }

    fn endpoint_url(
        &self,
        chain: &ChainDescriptor,
        service: &str,
        endpoint: &str,
    ) -> anyhow::Result<String> {
        let node = chain
            .nodes
            .first()
            .ok_or_else(|| anyhow!("Missing node in devnet descriptor"))?;
        let service = node
            .services
            .get(service)
            .ok_or_else(|| anyhow!("Missing {service} service in devnet descriptor"))?;
        let endpoint = service
            .endpoints
            .get(endpoint)
            .ok_or_else(|| anyhow!("Missing {endpoint} endpoint in devnet descriptor"))?;
        let scheme = endpoint.scheme.as_deref().unwrap_or("http");
        Ok(format!("{scheme}://{}:{}", endpoint.host, endpoint.port))
    }

    fn auxiliary_endpoint_url(
        &self,
        service: &str,
        endpoint: &str,
    ) -> anyhow::Result<Option<String>> {
        let service_name = service;
        let Some(service) = self.auxiliary_services.get(service_name) else {
            return Ok(None);
        };
        let endpoint = service.endpoints.get(endpoint).ok_or_else(|| {
            anyhow!("Missing {endpoint} endpoint for auxiliary service {service_name}")
        })?;
        let scheme = endpoint.scheme.as_deref().unwrap_or("http");
        Ok(Some(format!(
            "{scheme}://{}:{}",
            endpoint.host, endpoint.port
        )))
    }

    fn l1_rpc_url(&self) -> anyhow::Result<String> {
        self.endpoint_url(&self.l1, "el", "rpc")
    }

    fn l1_beacon_rpc_url(&self) -> anyhow::Result<String> {
        self.endpoint_url(&self.l1, "cl", "http")
    }

    fn l2_rpc_url(&self) -> anyhow::Result<String> {
        self.endpoint_url(self.l2_chain()?, "el", "rpc")
    }

    fn op_node_rpc_url(&self) -> anyhow::Result<String> {
        self.endpoint_url(self.l2_chain()?, "cl", "http")
    }

    fn eigenda_proxy_url(&self) -> anyhow::Result<String> {
        self.auxiliary_endpoint_url("eigenda_proxy", "http")?
            .ok_or_else(|| anyhow!("Missing eigenda_proxy endpoint in devnet descriptor"))
    }

    fn wallet(&self, alias: &str) -> anyhow::Result<&WalletDescriptor> {
        self.l1
            .wallets
            .get(alias)
            .ok_or_else(|| anyhow!("Missing wallet alias {alias} in devnet descriptor"))
    }

    fn private_key(&self, alias: &str) -> anyhow::Result<String> {
        Ok(self.wallet(alias)?.private_key.clone())
    }

    fn address(&self, alias: &str) -> anyhow::Result<String> {
        Ok(self.wallet(alias)?.address.clone())
    }

    fn provider_args(&self) -> anyhow::Result<ProviderArgs> {
        Ok(ProviderArgs {
            eth_rpc_url: self.l1_rpc_url()?,
            op_geth_url: self.l2_rpc_url()?,
            op_node_url: self.op_node_rpc_url()?,
            op_rpc_delay: 0,
            beacon_rpc_url: self.l1_beacon_rpc_url()?,
            op_rpc_concurrency: 64,
            rpc_poll_interval: 1,
            timeouts: Default::default(),
        })
    }
}

fn workspace_root() -> PathBuf {
    Path::new(WORKSPACE_ROOT)
        .join("../..")
        .canonicalize()
        .expect("Failed to resolve workspace root")
}

impl DevnetFlavor {
    const ALL: [Self; 2] = [Self::Standard, Self::EigenDA];

    fn descriptor_path(self) -> PathBuf {
        workspace_root().join(match self {
            Self::Standard => "devnet/kurtosis-devnet.json",
            Self::EigenDA => "devnet/kurtosis-eigenda-devnet.json",
        })
    }

    fn up_script(self) -> &'static str {
        match self {
            Self::Standard => "devnet-up.sh",
            Self::EigenDA => "devnet-up-eigenda.sh",
        }
    }

    fn clean_script(self) -> &'static str {
        match self {
            Self::Standard => "devnet-clean.sh",
            Self::EigenDA => "devnet-clean-eigenda.sh",
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::Standard => "Optimism",
            Self::EigenDA => "EigenDA",
        }
    }

    fn requires_eigenda_proxy(self) -> bool {
        matches!(self, Self::EigenDA)
    }
}

fn devnet_script(name: &str) -> PathBuf {
    workspace_root().join("scripts").join(name)
}

#[cfg(feature = "eigen")]
fn reserve_local_port() -> anyhow::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("Failed to reserve local port")?;
    let port = listener
        .local_addr()
        .context("Failed to read reserved local address")?
        .port();
    drop(listener);
    Ok(port)
}

async fn run_devnet_script(script: &str) -> io::Result<ExitStatus> {
    let script_path = devnet_script(script);
    let mut cmd = Command::new("/bin/bash");
    cmd.current_dir(workspace_root())
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .arg(&script_path);
    cmd.kill_on_drop(true)
        .spawn()
        .unwrap_or_else(|err| {
            panic!(
                "Failed to spawn devnet script {}: {err}",
                script_path.display()
            )
        })
        .wait()
        .await
}

async fn run_devnet(script: &str) -> anyhow::Result<()> {
    let exit_status = run_devnet_script(script).await?;
    if !exit_status.success() {
        return Err(anyhow!(
            "devnet script {script} failed with {exit_status:?}"
        ));
    }
    Ok(())
}

async fn wait_for_json_rpc(
    client: &Client,
    url: &str,
    method: &str,
    params: serde_json::Value,
) -> anyhow::Result<()> {
    let response = client
        .post(url)
        .json(&json!({
            "id": 1,
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
        .send()
        .await
        .with_context(|| format!("Failed to reach {url}"))?
        .error_for_status()
        .with_context(|| format!("Non-success response from {url}"))?;
    let payload: serde_json::Value = response
        .json()
        .await
        .with_context(|| format!("Invalid JSON-RPC response from {url}"))?;
    if let Some(error) = payload.get("error") {
        return Err(anyhow!("JSON-RPC error from {url}: {error}"));
    }
    Ok(())
}

async fn wait_for_beacon_api(client: &Client, url: &str) -> anyhow::Result<()> {
    client
        .get(format!("{url}/eth/v1/beacon/genesis"))
        .send()
        .await
        .with_context(|| format!("Failed to reach beacon endpoint {url}"))?
        .error_for_status()
        .with_context(|| format!("Non-success response from beacon endpoint {url}"))?;
    Ok(())
}

async fn wait_for_http_api(client: &Client, url: &str) -> anyhow::Result<()> {
    client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to reach HTTP endpoint {url}"))?
        .error_for_status()
        .with_context(|| format!("Non-success response from HTTP endpoint {url}"))?;
    Ok(())
}

async fn wait_for_devnet_ready(flavor: DevnetFlavor) -> anyhow::Result<DevnetConfig> {
    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("Failed to construct readiness HTTP client")?;
    let start = Instant::now();
    let descriptor_path = flavor.descriptor_path();

    loop {
        let current_error = match DevnetConfig::load(&descriptor_path) {
            Ok(config) => {
                let readiness: anyhow::Result<()> = async {
                    wait_for_json_rpc(&client, &config.l1_rpc_url()?, "eth_chainId", json!([]))
                        .await?;
                    wait_for_beacon_api(&client, &config.l1_beacon_rpc_url()?).await?;
                    wait_for_json_rpc(&client, &config.l2_rpc_url()?, "eth_chainId", json!([]))
                        .await?;
                    wait_for_json_rpc(
                        &client,
                        &config.op_node_rpc_url()?,
                        "optimism_outputAtBlock",
                        json!(["0x0"]),
                    )
                    .await?;
                    if flavor.requires_eigenda_proxy() {
                        wait_for_http_api(
                            &client,
                            &format!("{}/health", config.eigenda_proxy_url()?),
                        )
                        .await?;
                    }
                    Ok(())
                }
                .await;

                match readiness {
                    Ok(()) => return Ok(config),
                    Err(err) => err.to_string(),
                }
            }
            Err(err) if descriptor_path.exists() => err.to_string(),
            Err(_) => format!(
                "Waiting for devnet descriptor at {}",
                descriptor_path.display()
            ),
        };

        if start.elapsed() >= DEVNET_READINESS_TIMEOUT {
            return Err(anyhow!(
                "Timed out waiting for devnet readiness: {}",
                current_error
            ));
        }

        sleep(DEVNET_POLL_INTERVAL).await;
    }
}

async fn deploy_kailua_contracts(
    devnet: &DevnetConfig,
    challenge_timeout: u64,
) -> anyhow::Result<()> {
    // fast-track upgrade w/ devmode proof support
    set_var("RISC0_DEV_MODE", "1");
    set_var("RISC0_INFO", "1");
    fast_track(FastTrackArgs {
        eth_rpc_url: devnet.l1_rpc_url()?,
        op_geth_url: devnet.l2_rpc_url()?,
        op_node_url: devnet.op_node_rpc_url()?,
        txn_args: TransactArgs {
            txn_timeout: 12,
            exec_gas_premium: 0,
            blob_gas_premium: 0,
            eip_7594: false,
        },
        starting_block_number: 0,
        proposal_output_count: 5,
        output_block_span: 3,
        collateral_amount: 1,
        verifier_contract: None,
        challenge_timeout,
        proof_permit_timeout: 900,
        proof_permit_delay: 12,
        deployer_signer: DeployerSignerArgs::from(devnet.private_key(DEPLOYER_ALIAS)?),
        owner_signer: OwnerSignerArgs::from(devnet.private_key(OWNER_ALIAS)?),
        guardian_signer: Some(GuardianSignerArgs::from(
            devnet.private_key(GUARDIAN_ALIAS)?,
        )),
        vanguard_address: Some(devnet.address(VANGUARD_ALIAS)?),
        vanguard_advantage: Some(60),
        respect_kailua_proposals: true,
        telemetry: Default::default(),
        bypass_chain_registry: false,
        timeouts: Default::default(),
    })
    .await?;
    println!("Kailua contracts installed");
    Ok(())
}

async fn start_devnet_with_flavor(flavor: DevnetFlavor) -> anyhow::Result<DevnetConfig> {
    // print out INFO logs
    if let Err(err) = kona_cli::LogConfig::new(kona_cli::LogArgs {
        level: 3,
        stdout_quiet: false,
        stdout_format: Default::default(),
        file_directory: None,
        file_format: Default::default(),
        file_rotation: Default::default(),
    })
    .init_tracing_subscriber(None)
    {
        eprintln!("Failed to set up tracing: {err:?}");
    }
    run_devnet(flavor.up_script()).await?;
    let config = wait_for_devnet_ready(flavor).await?;
    println!("{} devnet deployed.", flavor.display_name());
    Ok(config)
}

async fn stop_devnet_with_flavor(flavor: DevnetFlavor) {
    match run_devnet_script(flavor.clean_script()).await {
        Ok(exit_code) => {
            println!("Cleanup Complete: {exit_code:?}")
        }
        Err(err) => {
            println!("Cleanup Error: {err:?}")
        }
    }
}

async fn stop_all_devnets() {
    for flavor in DevnetFlavor::ALL {
        stop_devnet_with_flavor(flavor).await;
    }
}

#[cfg(feature = "eigen")]
struct EigendaProxyShim {
    url: String,
    child: Child,
}

#[cfg(feature = "eigen")]
impl EigendaProxyShim {
    async fn start(upstream: &str) -> anyhow::Result<Self> {
        let port = reserve_local_port()?;
        let url = format!("http://127.0.0.1:{port}");
        let script_path = devnet_script("eigenda-proxy-encoded-shim.py");

        let mut cmd = Command::new("python3");
        cmd.current_dir(workspace_root())
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .arg(&script_path)
            .arg("--upstream")
            .arg(upstream)
            .arg("--listen-host")
            .arg("127.0.0.1")
            .arg("--listen-port")
            .arg(port.to_string());

        let mut child = cmd.kill_on_drop(true).spawn().unwrap_or_else(|err| {
            panic!(
                "Failed to spawn EigenDA proxy shim {}: {err}",
                script_path.display()
            )
        });

        let client = Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .context("Failed to construct EigenDA proxy shim readiness client")?;
        let health_url = format!("{url}/health");
        let start = Instant::now();
        loop {
            if let Some(status) = child
                .try_wait()
                .context("Failed to poll EigenDA proxy shim process")?
            {
                return Err(anyhow!(
                    "EigenDA proxy shim exited before becoming ready: {status:?}"
                ));
            }

            match wait_for_http_api(&client, &health_url).await {
                Ok(()) => return Ok(Self { url, child }),
                Err(err) => {
                    if start.elapsed() >= Duration::from_secs(30) {
                        return Err(anyhow!(
                            "Timed out waiting for EigenDA proxy shim readiness: {}",
                            err
                        ));
                    }
                }
            }

            sleep(DEVNET_POLL_INTERVAL).await;
        }
    }

    fn url(&self) -> &str {
        &self.url
    }
}

#[cfg(feature = "eigen")]
impl Drop for EigendaProxyShim {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

async fn start_clean_devnet_with_flavor(flavor: DevnetFlavor) -> anyhow::Result<DevnetConfig> {
    stop_all_devnets().await;
    match start_devnet_with_flavor(flavor).await {
        Ok(config) => Ok(config),
        Err(err) => {
            eprintln!("Error: {err}");
            stop_all_devnets().await;
            Err(err)
        }
    }
}

fn base_proving_args(max_witness_size: usize) -> ProvingArgs {
    ProvingArgs {
        payout_recipient_address: None,
        segment_limit: 21,
        max_derivation_length: u64::MAX,
        max_block_derivations: u64::MAX,
        max_block_executions: usize::MAX,
        max_proof_stitches: usize::MAX,
        max_witness_size,
        num_tail_blocks: 10,
        num_concurrent_preflights: 4,
        num_concurrent_proofs: 2,
        num_concurrent_witgens: None,
        num_concurrent_r0vm: None,
        bypass_chain_registry: false,
        skip_derivation_proof: false,
        skip_await_proof: false,
        clear_cache_data: true,
        #[cfg(feature = "eigen")]
        hokulea: Default::default(),
        #[cfg(feature = "celestia")]
        hana: Default::default(),
        export_profile_csv: false,
    }
}

async fn run_prover(
    devnet: &DevnetConfig,
    proof_size: u64,
    proving: ProvingArgs,
) -> anyhow::Result<()> {
    let tmp_dir = tempdir()?;
    let data_dir = tmp_dir.path().join("agent");
    let provider = devnet.provider_args()?;
    let sync = SyncArgs {
        provider,
        kailua_game_implementation: None,
        kailua_anchor_address: None,
        final_l2_block: Some(proof_size),
        data_dir: Some(data_dir.clone()),
        telemetry: Default::default(),
    };

    println!("Waiting for l2 block #{proof_size} to be safe.");
    let mut agent = SyncAgent::new(&sync.provider, data_dir.clone(), None, None, false).await?;
    loop {
        // Only op_rpc_concurrency/op_rpc_delay matter here; the agent
        // already holds the real provider URLs from its initial construction.
        agent
            .sync(&SyncArgs {
                provider: ProviderArgs {
                    op_rpc_concurrency: 64,
                    op_rpc_delay: 0,
                    ..Default::default()
                },
                final_l2_block: Some(proof_size),
                ..Default::default()
            })
            .await?;
        if agent.cursor.last_output_index >= proof_size {
            break;
        }
        sleep(Duration::from_secs(2)).await;
    }
    println!("Proving l2 block #{proof_size} since genesis");

    let l1_head = agent
        .provider
        .l1_provider
        .get_block(BlockId::Number(BlockNumberOrTag::Latest))
        .await?
        .ok_or_else(|| anyhow!("Missing latest L1 block"))?
        .header()
        .hash;
    let agreed_l2_head_hash = agent
        .provider
        .l2_provider
        .get_block(BlockId::Number(BlockNumberOrTag::Number(0)))
        .await?
        .ok_or_else(|| anyhow!("Missing L2 genesis block"))?
        .header()
        .hash;
    let agreed_l2_output_root = agent.provider.op_provider.output_at_block(0).await?;
    let claimed_l2_output_root = agent
        .provider
        .op_provider
        .output_at_block(proof_size)
        .await?;
    prove(ProveArgs {
        kona: kona_host::single::SingleChainHost {
            l1_head,
            agreed_l2_head_hash,
            agreed_l2_output_root,
            claimed_l2_output_root,
            claimed_l2_block_number: proof_size,
            l2_node_address: Some(sync.provider.op_geth_url.clone()),
            l1_node_address: Some(sync.provider.eth_rpc_url.clone()),
            l1_beacon_address: Some(sync.provider.beacon_rpc_url.clone()),
            data_dir: Some(tmp_dir.path().join("prover")),
            native: true,
            server: false,
            l2_chain_id: Some(agent.config.l2_chain_id.id()),
            rollup_config_path: None,
            l1_config_path: None,
            enable_experimental_witness_endpoint: false,
        },
        op_node_address: Some(sync.provider.op_node_url.clone()),
        proving,
        boundless: Default::default(),
        precondition_params: vec![],
        precondition_block_hashes: vec![],
        precondition_blob_hashes: vec![],
        telemetry: Default::default(),
        timeouts: Default::default(),
    })
    .await?;

    Ok(())
}

#[cfg(not(feature = "eigen"))]
#[tokio::test(flavor = "multi_thread")]
async fn proposer_validator() {
    // We can only run one of these dockerized devnets at a time
    let devnet_lock = DEVNET.lock().await;

    // Start the optimism devnet
    let devnet = start_clean_devnet_with_flavor(DevnetFlavor::Standard)
        .await
        .unwrap();
    // update dgf to use kailua
    deploy_kailua_contracts(&devnet, 60).await.unwrap();

    // Instantiate sync arguments
    let tmp_dir = tempdir().unwrap();
    let proposer_data_dir = tmp_dir.path().join("proposer").to_path_buf();
    let provider = devnet.provider_args().unwrap();
    let sync = SyncArgs {
        provider: provider.clone(),
        kailua_game_implementation: None,
        kailua_anchor_address: None,
        final_l2_block: Some(60),
        data_dir: Some(proposer_data_dir.clone()),
        telemetry: Default::default(),
    };

    // Instantiate transacting arguments
    let txn_args = TransactArgs {
        txn_timeout: 30,
        exec_gas_premium: 25,
        blob_gas_premium: 25,
        eip_7594: false,
    };

    // Instantiate proposer wallet
    let proposer_signer = ProposerSignerArgs::from(devnet.private_key(PROPOSER_ALIAS).unwrap());

    // Run the proposer until block 60
    propose(
        ProposeArgs {
            sync: sync.clone(),
            bypass_chain_registry: false,
            proposer_signer: proposer_signer.clone(),
            txn_args: txn_args.clone(),
        },
        proposer_data_dir.clone(),
    )
    .await
    .unwrap();

    // wait until block 75 is available
    let mut agent = SyncAgent::new(&sync.provider, proposer_data_dir.clone(), None, None, false)
        .await
        .unwrap();
    loop {
        agent
            .sync(&SyncArgs {
                provider: ProviderArgs {
                    op_rpc_concurrency: 64,
                    op_rpc_delay: 0,
                    ..Default::default()
                },
                final_l2_block: Some(75),
                ..Default::default()
            })
            .await
            .unwrap();
        if agent.cursor.last_output_index >= 75 {
            break;
        }
        // Wait for more blocks to be confirmed
        sleep(Duration::from_secs(2)).await;
    }
    // release proposer db
    let fault_parent = agent.cursor.last_resolved_game;
    drop(agent);

    // submit an output fault
    fault(FaultArgs {
        propose_args: ProposeArgs {
            sync: sync.clone(),
            bypass_chain_registry: false,
            proposer_signer: ProposerSignerArgs::from(
                devnet.private_key(FAULT_PROPOSER_ALIAS).unwrap(),
            ),
            txn_args: txn_args.clone(),
        },
        fault_offset: 1,
        fault_parent,
    })
    .await
    .unwrap();

    // submit a trail fault
    fault(FaultArgs {
        propose_args: ProposeArgs {
            sync: sync.clone(),
            bypass_chain_registry: false,
            proposer_signer: ProposerSignerArgs::from(
                devnet.private_key(TRAIL_FAULT_PROPOSER_ALIAS).unwrap(),
            ),
            txn_args: txn_args.clone(),
        },
        fault_offset: 250,
        fault_parent,
    })
    .await
    .unwrap();

    // new sync target at block 90
    let sync = SyncArgs {
        final_l2_block: Some(90),
        ..sync
    };

    // Run the proposer and validator until block 90
    let validator_data_dir = tmp_dir.path().join("validator").to_path_buf();
    let validator_handle = tokio::task::spawn(validate(
        ValidateArgs {
            sync: SyncArgs {
                data_dir: Some(validator_data_dir.clone()),
                ..sync.clone()
            },
            kailua_cli: None,
            fast_forward_start: 0,
            fast_forward_target: 0,
            num_concurrent_provers: 1,
            enable_experimental_witness_endpoint: true,
            max_fault_proving_delay: 0,
            max_validity_proving_delay: 0,
            fault_proving_permit: PermitPolicy::MANDATORY,
            fault_proving_permit_expiry: 600,
            min_validity_proving_timestamp: 0,
            l1_head_jump_back: 0,
            validator_signer: ValidatorSignerArgs::from(
                devnet.private_key(VALIDATOR_ALIAS).unwrap(),
            ),
            txn_args: txn_args.clone(),
            proving: ProvingArgs {
                num_concurrent_preflights: 1,
                num_concurrent_proofs: 1,
                bypass_chain_registry: true,
                ..base_proving_args(2_684_354_560)
            },
            boundless: Default::default(),
        },
        3,
        validator_data_dir.clone(),
    ));
    let proposer_handle = tokio::task::spawn(propose(
        ProposeArgs {
            sync: sync.clone(),
            bypass_chain_registry: false,
            proposer_signer: proposer_signer.clone(),
            txn_args: txn_args.clone(),
        },
        proposer_data_dir.clone(),
    ));
    println!("Waiting for proposer and validator to terminate.");
    // Wait for both agents to hit termination condition
    let (validator, proposer) = try_join!(validator_handle, proposer_handle).unwrap();
    validator.unwrap();
    proposer.unwrap();

    // Deploy new set of Kailua contracts for validity proving
    deploy_kailua_contracts(&devnet, u64::MAX).await.unwrap();
    // Run the proposer and validator until block 90
    let validator_data_dir = tmp_dir.path().join("validator").to_path_buf();
    let validator_handle = tokio::task::spawn(validate(
        ValidateArgs {
            sync: SyncArgs {
                data_dir: Some(validator_data_dir.clone()),
                ..sync.clone()
            },
            kailua_cli: None,
            fast_forward_start: 0,
            fast_forward_target: 90, // run validity proofs until block 90 is finalized
            num_concurrent_provers: 5,
            enable_experimental_witness_endpoint: true,
            max_fault_proving_delay: 0,
            max_validity_proving_delay: 0,
            fault_proving_permit: PermitPolicy::MANDATORY,
            fault_proving_permit_expiry: 600,
            min_validity_proving_timestamp: 0,
            l1_head_jump_back: 0,
            validator_signer: ValidatorSignerArgs::from(
                devnet.private_key(VALIDATOR_ALIAS).unwrap(),
            ),
            txn_args: txn_args.clone(),
            proving: ProvingArgs {
                num_concurrent_preflights: 1,
                num_concurrent_proofs: 1,
                bypass_chain_registry: true,
                ..base_proving_args(2_684_354_560)
            },
            boundless: Default::default(),
        },
        3,
        validator_data_dir.clone(),
    ));
    let proposer_handle = tokio::task::spawn(propose(
        ProposeArgs {
            sync: sync.clone(),
            bypass_chain_registry: false,
            proposer_signer: proposer_signer.clone(),
            txn_args: txn_args.clone(),
        },
        proposer_data_dir.clone(),
    ));
    println!("Waiting for proposer and validator to terminate.");
    // Wait for both agents to hit termination condition
    let (validator, proposer) = try_join!(validator_handle, proposer_handle).unwrap();
    validator.unwrap();
    proposer.unwrap();

    // Stop and discard the devnet
    stop_all_devnets().await;
    drop(devnet_lock);
}

#[cfg(not(feature = "eigen"))]
#[tokio::test(flavor = "multi_thread")]
async fn prover() {
    const PROOF_SIZE: u64 = 200;

    // We can only run one of these dockerized devnets at a time
    let devnet_lock = DEVNET.lock().await;

    // Start the optimism devnet
    let devnet = start_clean_devnet_with_flavor(DevnetFlavor::Standard)
        .await
        .unwrap();
    // update dgf to use kailua
    deploy_kailua_contracts(&devnet, 60).await.unwrap();

    run_prover(&devnet, PROOF_SIZE, base_proving_args(5 * 1024 * 1024))
        .await
        .unwrap();

    // Stop and discard the devnet
    stop_all_devnets().await;
    drop(devnet_lock);
}

#[cfg(feature = "eigen")]
#[tokio::test(flavor = "multi_thread")]
async fn prover_hokulea() {
    const PROOF_SIZE: u64 = 60;

    let devnet_lock = DEVNET.lock().await;

    let devnet = start_clean_devnet_with_flavor(DevnetFlavor::EigenDA)
        .await
        .unwrap();
    deploy_kailua_contracts(&devnet, 60).await.unwrap();
    let eigenda_proxy_shim = EigendaProxyShim::start(&devnet.eigenda_proxy_url().unwrap())
        .await
        .unwrap();
    let mut proving = base_proving_args(5 * 1024 * 1024);
    proving.hokulea.eigenda_proxy_address = Some(eigenda_proxy_shim.url().to_owned());

    run_prover(&devnet, PROOF_SIZE, proving).await.unwrap();

    drop(eigenda_proxy_shim);
    stop_all_devnets().await;
    drop(devnet_lock);
}

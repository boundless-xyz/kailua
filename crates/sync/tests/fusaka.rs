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

use alloy::consensus::Blob;
use alloy::network::{Ethereum, TransactionBuilder4844};
use alloy::primitives::Address;
use alloy::providers::Provider;
use clap::Parser;
use kailua_sync::provider::beacon::blob_sidecar;
use kailua_sync::transact::provider::KailuaProvider;
use kailua_sync::transact::signer::ProposerSignerArgs;
use kailua_sync::transact::TransactArgs;
use std::ffi::OsString;
use std::time::Duration;
use tracing::info;

#[derive(clap::Parser, Debug, Clone)]
pub struct TestArgs {
    /// Address of the ethereum rpc endpoint to use (eth namespace required)
    #[clap(long, env)]
    pub eth_rpc_url: String,

    /// L1 wallet to use for proposing outputs
    #[clap(flatten)]
    pub proposer_signer: ProposerSignerArgs,

    /// Transaction publication configuration
    #[clap(flatten)]
    pub txn_args: TransactArgs,

    /// Capture unused cargo test args
    #[clap(trailing_var_arg = true)]
    pub extra_args: Vec<OsString>,
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn publish_cell_proofs_txn() {
    let _ = unsafe { blst::blst_p1_sizeof() };

    let args = TestArgs::parse();
    kona_cli::LogConfig::new(kona_cli::LogArgs {
        level: 3,
        stdout_quiet: false,
        stdout_format: Default::default(),
        file_directory: None,
        file_format: Default::default(),
        file_rotation: Default::default(),
    })
    .init_tracing_subscriber(None)
    .unwrap();

    // initialize proposer wallet
    info!("Initializing proposer wallet.");
    let proposer_wallet = args
        .proposer_signer
        .wallet(None)
        .await
        .expect("Proposer signer wallet setup");
    let proposer_address = proposer_wallet.default_signer().address();
    let proposer_provider = KailuaProvider::new(
        args.txn_args
            .premium_provider::<Ethereum>()
            .wallet(&proposer_wallet)
            .connect_http(
                args.eth_rpc_url
                    .as_str()
                    .try_into()
                    .expect("incorrect eth_rpc_url"),
            ),
        args.txn_args.eip_7594,
    );
    info!("Proposer address: {proposer_address}");

    // create a dummy EIP-4844 transaction that will be converted to EIP-7594 by KailuaProvider
    let transaction = proposer_provider
        .transaction_request()
        .to(Address::ZERO)
        .with_blob_sidecar(blob_sidecar(vec![Blob::new([0x01; 131072])]).expect("blob_sidecar"));

    // wait for transaction submission
    // Publish transaction
    let pending_txn = proposer_provider
        .send_transaction(transaction)
        .await
        .expect("send");
    info!("Transaction published: {:?}", pending_txn.tx_hash());

    // Wait for receipt with timeout
    let receipt = pending_txn
        .with_timeout(Some(Duration::from_secs(args.txn_args.txn_timeout)))
        .get_receipt()
        .await
        .expect("get_receipt");

    let blob_gas = receipt.blob_gas_used.expect("Blobs not published!");
    info!("Blob gas: {blob_gas}");
}

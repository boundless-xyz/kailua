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

/// Gas-premium transaction fillers.
pub mod fillers;
/// Fee-escalating transaction-publishing provider.
pub mod provider;
/// Retried block queries.
pub mod rpc;
/// Transaction execution through a Gnosis Safe.
pub mod safe;
/// Signer configuration from raw keys, AWS KMS, or GCP KMS.
pub mod signer;

use alloy::contract::{CallBuilder, CallDecoder, EthCall};
use alloy::network::{Network, TransactionBuilder4844};
use alloy::providers::Provider;
use alloy::providers::fillers::JoinFill;
use alloy::providers::{Identity, ProviderBuilder};
use anyhow::Context;
use async_trait::async_trait;
use fillers::{PremiumBlobGasFiller, PremiumExecGasFiller, PremiumFiller};
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use std::future::IntoFuture;
use std::time::Duration;
use tracing::info;

/// Transaction publication parameters.
#[derive(clap::Args, Debug, Clone)]
pub struct TransactArgs {
    /// Transaction Confirmation Timeout
    #[clap(long, env, required = false, default_value_t = 120)]
    pub txn_timeout: u64,
    /// Execution Gas Fee Premium
    #[clap(long, env, required = false, default_value_t = 25)]
    pub exec_gas_premium: u128,
    /// Blob Gas Fee Premium
    #[clap(long, env, required = false, default_value_t = 25)]
    pub blob_gas_premium: u128,
    /// Whether to apply EIP-7594 encoding to EIP-4844 transactions
    #[clap(long, env, required = false, default_value_t = false)]
    pub eip_7594: bool,
}

impl TransactArgs {
    /// Returns a provider builder applying the configured gas fee premiums (see
    /// [premium_provider]).
    pub fn premium_provider<N: Network>(
        &self,
    ) -> ProviderBuilder<Identity, JoinFill<Identity, PremiumFiller>>
    where
        N::TransactionRequest: TransactionBuilder4844,
    {
        premium_provider::<N>(self.exec_gas_premium, self.blob_gas_premium)
    }
}

/// Publication of contract calls as transactions, instrumented under a tracing span.
#[async_trait]
pub trait Transact<N: Network> {
    /// Publishes the call as a transaction and awaits its receipt within the timeout.
    async fn transact(
        &self,
        span: &'static str,
        timeout: Option<Duration>,
    ) -> anyhow::Result<N::ReceiptResponse>;

    /// [Self::transact] under the given telemetry context.
    async fn timed_transact_with_context(
        &self,
        context: opentelemetry::Context,
        span: &'static str,
        timeout: Option<Duration>,
    ) -> anyhow::Result<N::ReceiptResponse> {
        self.transact(span, timeout).with_context(context).await
    }

    /// [Self::transact] under the given telemetry context, with no receipt timeout.
    async fn transact_with_context(
        &self,
        context: opentelemetry::Context,
        span: &'static str,
    ) -> anyhow::Result<N::ReceiptResponse> {
        self.timed_transact_with_context(context, span, None).await
    }
}

#[async_trait]
impl<'coder, P: Provider<N>, D: CallDecoder + Send + Sync + 'static, N: Network> Transact<N>
    for CallBuilder<P, D, N>
where
    CallBuilder<P, D, N>: Clone,
    EthCall<'coder, D, N>: IntoFuture,
{
    async fn transact(
        &self,
        span: &'static str,
        timeout: Option<Duration>,
    ) -> anyhow::Result<N::ReceiptResponse> {
        let tracer = tracer("kailua");
        let context = opentelemetry::Context::current_with_span(tracer.start(span));

        // Publish transaction
        let pending_txn = self
            .send()
            .with_context(context.with_span(tracer.start_with_context("send", &context)))
            .await
            .context("send")?;
        info!("Transaction published: {:?}", pending_txn.tx_hash());

        // Wait for receipt with timeout
        pending_txn
            .with_timeout(timeout)
            .get_receipt()
            .with_context(context.with_span(tracer.start_with_context("get_receipt", &context)))
            .await
            .context("get_receipt")
    }
}

/// Returns a provider builder whose fillers mark up estimated execution and blob gas prices
/// by the given percentages, fetch nonces from the latest block, and fill in the chain id.
pub fn premium_provider<N: Network>(
    premium_exec_gas: u128,
    premium_blob_gas: u128,
) -> ProviderBuilder<Identity, JoinFill<Identity, PremiumFiller>>
where
    N::TransactionRequest: TransactionBuilder4844,
{
    ProviderBuilder::default().filler(JoinFill::new(
        PremiumExecGasFiller::with_premium(premium_exec_gas),
        JoinFill::new(
            PremiumBlobGasFiller::with_premium(premium_blob_gas),
            Default::default(),
        ),
    ))
}

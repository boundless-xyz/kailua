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

use crate::await_tel;
use crate::transact::rpc::get_block;
use alloy::consensus::transaction::SignerRecoverable;
use alloy::consensus::{
    BlobTransactionSidecar, BlockHeader, Bytes48, EthereumTxEnvelope, Signed, Transaction,
    TxEip4844Variant, TxEip4844WithSidecar,
};
use alloy::eips::eip7594::BlobTransactionSidecarEip7594;
use alloy::eips::{BlockId, BlockNumberOrTag, Encodable2718};
use alloy::network::{BlockResponse, Ethereum, Network, TransactionBuilder4844};
use alloy::providers::fillers::{FillProvider, TxFiller};
use alloy::providers::network::TransactionBuilder;
use alloy::providers::{PendingTransactionBuilder, Provider, RootProvider};
use alloy::transports::TransportResult;
use async_trait::async_trait;
use itertools::Itertools;
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use tracing::info;

#[derive(Debug, Clone)]
pub struct KailuaProvider<P> {
    /// Inner provider.
    inner: P,

    /// Whether to use EIP-7594
    eip_7594: bool,
}

impl<P> KailuaProvider<P> {
    pub fn new(inner: P, eip_7594: bool) -> Self {
        Self { inner, eip_7594 }
    }

    pub fn provider(&self) -> &P {
        &self.inner
    }
}

#[async_trait]
impl<F: TxFiller<Ethereum>, P: Provider<Ethereum>> Provider<Ethereum>
    for KailuaProvider<FillProvider<F, P, Ethereum>>
where
    P: Provider<Ethereum>,
{
    fn root(&self) -> &RootProvider<Ethereum> {
        self.inner.root()
    }

    async fn send_transaction(
        &self,
        tx: <Ethereum as Network>::TransactionRequest,
    ) -> TransportResult<PendingTransactionBuilder<Ethereum>> {
        let mut fee_factor = 1.0;
        let tracer = tracer("kailua");
        let context = opentelemetry::Context::current_with_span(
            tracer.start("Proposal::fetch_current_challenger_duration"),
        );

        // Recover signer and fill fees
        let envelope = self
            .inner
            .fill(tx.clone())
            .await?
            .as_envelope()
            .cloned()
            .unwrap();
        let sender = envelope.recover_signer().unwrap();
        let max_priority_fee_per_gas = envelope.max_priority_fee_per_gas();
        let max_fee_per_blob_gas = envelope.max_fee_per_blob_gas();
        let max_fee_per_gas = envelope.max_fee_per_gas();

        loop {
            let mut tx = tx.clone();
            // Get latest block
            let latest_block = await_tel!(
                context,
                get_block(self.provider(), BlockNumberOrTag::Latest, 12)
            )
            .header()
            .number();
            info!("Testing transaction viability under block {latest_block}");

            // Ensure call success
            self.call(tx.clone().with_from(sender))
                .block(BlockId::Number(BlockNumberOrTag::Number(latest_block)))
                .await?;

            // Set nonce to that as of successful call block
            tx.set_nonce(
                self.inner
                    .get_transaction_count(sender)
                    .block_id(BlockId::Number(BlockNumberOrTag::Number(latest_block)))
                    .await?,
            );

            info!(
                "Broadcasting transaction with nonce {} and fee factor {fee_factor}",
                tx.nonce.unwrap_or_default()
            );

            // scale fees
            if let Some(fee) = max_priority_fee_per_gas {
                tx.set_max_priority_fee_per_gas((fee as f64 * fee_factor) as u128);
            }
            if let Some(fee) = max_fee_per_blob_gas {
                tx.set_max_fee_per_blob_gas((fee as f64 * fee_factor) as u128);
            }
            tx.set_max_fee_per_gas((max_fee_per_gas as f64 * fee_factor) as u128);

            // Sign transaction
            let envelope = self.inner.fill(tx).await?.try_into_envelope().unwrap();

            // EIP-7594 Patch (todo: remove once alloy-rs is fixed)
            let has_sidecar = envelope
                .as_eip4844()
                .map(|tx| tx.tx().sidecar().is_some())
                .unwrap_or_default();
            let encoded_tx = if self.eip_7594 && has_sidecar {
                info!("Applying EIP-7594 to EIP-4844 transaction.");
                EthereumTxEnvelope::Eip4844(Signed::new_unhashed(
                    TxEip4844Variant::TxEip4844WithSidecar(
                        TxEip4844WithSidecar::from_tx_and_sidecar(
                            envelope.as_eip4844().unwrap().tx().tx().clone(),
                            convert_sidecar(
                                envelope
                                    .as_eip4844()
                                    .unwrap()
                                    .tx()
                                    .sidecar()
                                    .unwrap()
                                    .clone(),
                            ),
                        ),
                    ),
                    *envelope.signature(),
                ))
                .encoded_2718()
            } else {
                envelope.encoded_2718()
            };

            // attempt broadcast
            match self.inner.send_raw_transaction(&encoded_tx).await {
                Ok(res) => break Ok(res),
                Err(err) => {
                    if !err.to_string().contains("underpriced") {
                        break Err(err);
                    }
                    // increase fees
                    fee_factor *= 1.1;
                }
            }
        }
    }
}

pub fn convert_sidecar(sidecar: BlobTransactionSidecar) -> BlobTransactionSidecarEip7594 {
    let settings = alloy::consensus::EnvKzgSettings::default();
    let cell_proofs = sidecar
        .blobs
        .iter()
        .map(|b| {
            let c_kzg_blob = c_kzg::Blob::from_bytes(b.as_slice()).unwrap();
            let (_, proofs) = settings
                .get()
                .compute_cells_and_kzg_proofs(&c_kzg_blob)
                .unwrap();
            proofs
                .into_iter()
                .map(|p| Bytes48::from(p.to_bytes().into_inner()))
                .collect::<Vec<_>>()
        })
        .concat();
    BlobTransactionSidecarEip7594 {
        blobs: sidecar.blobs,
        commitments: sidecar.commitments,
        cell_proofs,
    }
}

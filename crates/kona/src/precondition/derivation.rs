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

use crate::config;
use crate::config::genesis_system_config_hash;
use crate::driver::{
    CachedAttributesQueueStage, CachedBatchProvider, CachedBatchQueue, CachedBatchStream,
    CachedBatchValidator, CachedChannelAssembler, CachedChannelBank, CachedChannelProvider,
    CachedChannelReader, CachedDerivationPipeline, CachedDriver, CachedFrameQueue,
    CachedL1Retrieval, CachedPollingTraversal,
};
use crate::rkyv::driver::sorted_by_key;
use alloy_eips::eip4895::Withdrawal;
use alloy_eips::Typed2718;
use alloy_primitives::Bytes;
use alloy_rpc_types_engine::PayloadAttributes;
use kona_driver::PipelineCursor;
use kona_executor::BlockBuildingOutcome;
use kona_protocol::{
    Batch, BatchWithInclusionBlock, BlockInfo, Channel, Frame, L2BlockInfo, OpAttributesWithParent,
    SingleBatch, SpanBatch, SpanBatchElement, SpanBatchTransactions,
};
use op_alloy_rpc_types_engine::OpPayloadAttributes;
use risc0_zkvm::sha::{Digestible, Impl as SHA2, Sha256};
use risc0_zkvm::Digest;

pub fn flatten_pipeline_cursor(pipeline_cursor: &PipelineCursor) -> Vec<u8> {
    [
        (pipeline_cursor.capacity as u64).to_be_bytes().as_slice(),
        pipeline_cursor.channel_timeout.to_be_bytes().as_slice(),
        flatten_block_info(&pipeline_cursor.origin).as_slice(),
        (pipeline_cursor.origins.len() as u64)
            .to_be_bytes()
            .as_slice(),
        pipeline_cursor
            .origins
            .iter()
            .map(|o| o.to_be_bytes())
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
        (pipeline_cursor.origin_infos.len() as u64)
            .to_be_bytes()
            .as_slice(),
        sorted_by_key(
            pipeline_cursor
                .origin_infos
                .clone()
                .into_iter()
                .collect::<Vec<_>>(),
        )
        .iter()
        .map(|(k, v)| [k.to_be_bytes().as_slice(), flatten_block_info(v).as_slice()].concat())
        .collect::<Vec<_>>()
        .concat()
        .as_slice(),
        (pipeline_cursor.tips.len() as u64).to_be_bytes().as_slice(),
        pipeline_cursor
            .tips
            .iter()
            .map(|(k, v)| {
                [
                    k.to_be_bytes().as_slice(),
                    flatten_l2_block_info(&v.l2_safe_head).as_slice(),
                    v.l2_safe_head_header.hash_slow().as_slice(),
                    v.l2_safe_head_output_root.as_slice(),
                ]
                .concat()
            })
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
    ]
    .concat()
}

pub fn flatten_safe_head_artifacts(artifacts: &(BlockBuildingOutcome, Vec<Bytes>)) -> Vec<u8> {
    [
        flatten_block_build_outcome(&artifacts.0).as_slice(),
        (artifacts.1.len() as u64).to_be_bytes().as_slice(),
        artifacts
            .1
            .iter()
            .map(flatten_bytes)
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
    ]
    .concat()
}

pub fn flatten_block_build_outcome(outcome: &BlockBuildingOutcome) -> Vec<u8> {
    [
        outcome.header.hash_slow().as_slice(),
        (outcome.execution_result.receipts.len() as u64)
            .to_be_bytes()
            .as_slice(),
        outcome
            .execution_result
            .receipts
            .iter()
            .map(alloy_rlp::encode)
            .map(flatten_bytes)
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
        (outcome.execution_result.requests.len() as u64)
            .to_be_bytes()
            .as_slice(),
        outcome
            .execution_result
            .requests
            .iter()
            .map(flatten_bytes)
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
        outcome.execution_result.gas_used.to_be_bytes().as_slice(),
        outcome
            .execution_result
            .blob_gas_used
            .to_be_bytes()
            .as_slice(),
    ]
    .concat()
}

pub fn flatten_payload_attributes(payload_attributes: &PayloadAttributes) -> Vec<u8> {
    [
        payload_attributes.timestamp.to_be_bytes().as_slice(),
        payload_attributes.prev_randao.as_slice(),
        payload_attributes.suggested_fee_recipient.as_slice(),
        payload_attributes
            .withdrawals
            .as_ref()
            .map(|v| v.len() as u64)
            .unwrap_or_default()
            .to_be_bytes()
            .as_slice(),
        payload_attributes
            .withdrawals
            .as_ref()
            .map(|v| v.iter().map(flatten_withdrawal).collect::<Vec<_>>())
            .unwrap_or_default()
            .concat()
            .as_slice(),
        payload_attributes
            .parent_beacon_block_root
            .unwrap_or_default()
            .as_slice(),
    ]
    .concat()
}

pub fn flatten_op_payload_attributes(op_payload_attributes: &OpPayloadAttributes) -> Vec<u8> {
    [
        flatten_payload_attributes(&op_payload_attributes.payload_attributes).as_slice(),
        op_payload_attributes
            .transactions
            .as_ref()
            .map(|v| v.len() as u64)
            .unwrap_or_default()
            .to_be_bytes()
            .as_slice(),
        op_payload_attributes
            .transactions
            .as_ref()
            .map(|v| v.iter().map(flatten_bytes).collect::<Vec<_>>())
            .unwrap_or_default()
            .concat()
            .as_slice(),
        config::opt_byte_arr(op_payload_attributes.no_tx_pool.map(|v| [v as u8])).as_slice(),
        config::opt_byte_arr(op_payload_attributes.gas_limit.map(|v| v.to_be_bytes())).as_slice(),
        config::opt_byte_arr(op_payload_attributes.eip_1559_params.map(|v| v.0)).as_slice(),
        config::opt_byte_arr(op_payload_attributes.min_base_fee.map(|v| v.to_be_bytes()))
            .as_slice(),
    ]
    .concat()
}

pub fn flatten_op_attrib_with_parent(op_attrib_with_parent: &OpAttributesWithParent) -> Vec<u8> {
    [
        flatten_op_payload_attributes(&op_attrib_with_parent.attributes).as_slice(),
        flatten_l2_block_info(&op_attrib_with_parent.parent).as_slice(),
        config::opt_byte_vec(
            op_attrib_with_parent
                .derived_from
                .as_ref()
                .map(flatten_block_info),
        )
        .as_slice(),
        &[op_attrib_with_parent.is_last_in_span as u8],
    ]
    .concat()
}

pub fn flatten_withdrawal(withdrawal: &Withdrawal) -> Vec<u8> {
    [
        withdrawal.index.to_be_bytes().as_slice(),
        withdrawal.validator_index.to_be_bytes().as_slice(),
        withdrawal.address.as_slice(),
        withdrawal.amount.to_be_bytes().as_slice(),
    ]
    .concat()
}

pub fn flatten_l2_block_info(l2_block_info: &L2BlockInfo) -> Vec<u8> {
    [
        flatten_block_info(&l2_block_info.block_info).as_slice(),
        l2_block_info.l1_origin.number.to_be_bytes().as_slice(),
        l2_block_info.l1_origin.hash.as_slice(),
        l2_block_info.seq_num.to_be_bytes().as_slice(),
    ]
    .concat()
}

pub fn flatten_batch_with_inclusion_block(
    batch_with_inclusion_block: &BatchWithInclusionBlock,
) -> Vec<u8> {
    [
        flatten_block_info(&batch_with_inclusion_block.inclusion_block).as_slice(),
        flatten_batch(&batch_with_inclusion_block.batch).as_slice(),
    ]
    .concat()
}

pub fn flatten_batch(batch: &Batch) -> Vec<u8> {
    match batch {
        Batch::Single(single_batch) => {
            [&[0xF1], flatten_single_batch(single_batch).as_slice()].concat()
        }
        Batch::Span(span_batch) => [&[0xF2], flatten_span_batch(span_batch).as_slice()].concat(),
    }
}

pub fn flatten_span_batch(span_batch: &SpanBatch) -> Vec<u8> {
    [
        span_batch.parent_check.as_slice(),
        span_batch.l1_origin_check.as_slice(),
        span_batch.genesis_timestamp.to_be_bytes().as_slice(),
        span_batch.chain_id.to_be_bytes().as_slice(),
        (span_batch.batches.len() as u64).to_be_bytes().as_slice(),
        span_batch
            .batches
            .iter()
            .map(flatten_span_batch_element)
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
        flatten_bytes(span_batch.origin_bits.as_ref()).as_slice(),
        (span_batch.block_tx_counts.len() as u64)
            .to_be_bytes()
            .as_slice(),
        span_batch
            .block_tx_counts
            .iter()
            .map(|v| v.to_be_bytes())
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
        flatten_span_batch_transactions(&span_batch.txs).as_slice(),
    ]
    .concat()
}

pub fn flatten_span_batch_transactions(span_batch_transactions: &SpanBatchTransactions) -> Vec<u8> {
    [
        span_batch_transactions
            .total_block_tx_count
            .to_be_bytes()
            .as_slice(),
        flatten_bytes(span_batch_transactions.contract_creation_bits.as_ref()).as_slice(),
        (span_batch_transactions.tx_sigs.len() as u64)
            .to_be_bytes()
            .as_slice(),
        span_batch_transactions
            .tx_sigs
            .iter()
            .map(|s| s.as_bytes())
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
        (span_batch_transactions.tx_nonces.len() as u64)
            .to_be_bytes()
            .as_slice(),
        span_batch_transactions
            .tx_nonces
            .iter()
            .map(|v| v.to_be_bytes())
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
        (span_batch_transactions.tx_gases.len() as u64)
            .to_be_bytes()
            .as_slice(),
        span_batch_transactions
            .tx_gases
            .iter()
            .map(|v| v.to_be_bytes())
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
        (span_batch_transactions.tx_tos.len() as u64)
            .to_be_bytes()
            .as_slice(),
        span_batch_transactions
            .tx_tos
            .iter()
            .map(|a| *a.0)
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
        (span_batch_transactions.tx_data.len() as u64)
            .to_be_bytes()
            .as_slice(),
        span_batch_transactions
            .tx_data
            .iter()
            .map(flatten_bytes)
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
        flatten_bytes(span_batch_transactions.protected_bits.as_ref()).as_slice(),
        (span_batch_transactions.tx_types.len() as u64)
            .to_be_bytes()
            .as_slice(),
        span_batch_transactions
            .tx_types
            .iter()
            .map(|v| v.ty())
            .collect::<Vec<_>>()
            .as_slice(),
        span_batch_transactions
            .legacy_tx_count
            .to_be_bytes()
            .as_slice(),
    ]
    .concat()
}

pub fn flatten_span_batch_element(span_batch_element: &SpanBatchElement) -> Vec<u8> {
    [
        span_batch_element.epoch_num.to_be_bytes().as_slice(),
        span_batch_element.timestamp.to_be_bytes().as_slice(),
        span_batch_element
            .transactions
            .iter()
            .map(flatten_bytes)
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
    ]
    .concat()
}

pub fn flatten_single_batch(single_batch: &SingleBatch) -> Vec<u8> {
    [
        single_batch.parent_hash.as_slice(),
        single_batch.epoch_num.to_be_bytes().as_slice(),
        single_batch.epoch_hash.as_slice(),
        single_batch.timestamp.to_be_bytes().as_slice(),
        single_batch
            .transactions
            .iter()
            .map(flatten_bytes)
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
    ]
    .concat()
}

pub fn flatten_bytes(bytes: impl AsRef<[u8]>) -> Vec<u8> {
    let bytes = bytes.as_ref();
    [(bytes.len() as u64).to_be_bytes().as_slice(), bytes].concat()
}

pub fn flatten_channel(channel: &Channel) -> Vec<u8> {
    let inputs = sorted_by_key(
        channel
            .inputs
            .iter()
            .map(|(k, v)| (*k, flatten_frame(v)))
            .collect(),
    )
    .into_iter()
    .map(|(_, v)| v)
    .collect::<Vec<_>>()
    .concat();
    [
        channel.id.as_slice(),
        flatten_block_info(&channel.open_block).as_slice(),
        (channel.estimated_size as u64).to_be_bytes().as_slice(),
        &[channel.closed as u8],
        channel.highest_frame_number.to_be_bytes().as_slice(),
        channel.last_frame_number.to_be_bytes().as_slice(),
        inputs.as_slice(),
        flatten_block_info(&channel.highest_l1_inclusion_block).as_slice(),
    ]
    .concat()
}

pub fn flatten_frame(frame: &Frame) -> Vec<u8> {
    [
        &frame.id,
        frame.number.to_be_bytes().as_slice(),
        frame.data.as_slice(),
        &[frame.is_last as u8],
    ]
    .concat()
}

pub fn flatten_block_info(block_info: &BlockInfo) -> Vec<u8> {
    [
        block_info.hash.as_slice(),
        block_info.number.to_be_bytes().as_slice(),
        block_info.parent_hash.as_slice(),
        block_info.timestamp.to_be_bytes().as_slice(),
    ]
    .concat()
}

impl Digestible for CachedDriver {
    fn digest(&self) -> Digest {
        let fields = [
            &[0x0C],
            flatten_pipeline_cursor(&self.cursor).digest().as_bytes(),
            self.safe_head_artifacts
                .as_ref()
                .map(flatten_safe_head_artifacts)
                .digest()
                .as_bytes(),
            self.pipeline.digest().as_bytes(),
        ]
        .concat();
        *SHA2::hash_bytes(fields.as_slice())
    }
}

impl Digestible for CachedDerivationPipeline {
    fn digest(&self) -> Digest {
        let fields = [
            &[0x0B],
            self.prepared
                .iter()
                .map(flatten_op_attrib_with_parent)
                .collect::<Vec<_>>()
                .digest()
                .as_bytes(),
            self.attributes.digest().as_bytes(),
        ]
        .concat();
        *SHA2::hash_bytes(fields.as_slice())
    }
}

impl Digestible for CachedAttributesQueueStage {
    fn digest(&self) -> Digest {
        let fields = [
            &[0x0A],
            &[self.is_last_in_span as u8],
            self.batch
                .as_ref()
                .map(flatten_single_batch)
                .digest()
                .as_bytes(),
            self.prev.digest().as_bytes(),
        ]
        .concat();
        *SHA2::hash_bytes(fields.as_slice())
    }
}

impl Digestible for CachedBatchProvider {
    fn digest(&self) -> Digest {
        match self {
            CachedBatchProvider::None => Digest::default(),
            CachedBatchProvider::BatchStream(bs) => bs.digest(),
            CachedBatchProvider::BatchQueue(bq) => bq.digest(),
            CachedBatchProvider::BatchValidator(bv) => bv.digest(),
        }
    }
}

impl Digestible for CachedBatchQueue {
    fn digest(&self) -> Digest {
        let fields = [
            &[0x09],
            self.origin
                .as_ref()
                .map(flatten_block_info)
                .unwrap_or_default()
                .digest()
                .as_bytes(),
            self.l1_blocks
                .iter()
                .map(flatten_block_info)
                .collect::<Vec<_>>()
                .digest()
                .as_bytes(),
            self.batches
                .iter()
                .map(flatten_batch_with_inclusion_block)
                .collect::<Vec<_>>()
                .digest()
                .as_bytes(),
            self.next_spans
                .iter()
                .map(flatten_single_batch)
                .collect::<Vec<_>>()
                .digest()
                .as_bytes(),
            self.prev.digest().as_bytes(),
        ]
        .concat();
        *SHA2::hash_bytes(fields.as_slice())
    }
}

impl Digestible for CachedBatchValidator {
    fn digest(&self) -> Digest {
        let fields = [
            &[0x08],
            self.origin
                .as_ref()
                .map(flatten_block_info)
                .unwrap_or_default()
                .digest()
                .as_bytes(),
            self.l1_blocks
                .iter()
                .map(flatten_block_info)
                .collect::<Vec<_>>()
                .digest()
                .as_bytes(),
            self.prev.digest().as_bytes(),
        ]
        .concat();
        *SHA2::hash_bytes(fields.as_slice())
    }
}

impl Digestible for CachedBatchStream {
    fn digest(&self) -> Digest {
        let buffer = self
            .buffer
            .iter()
            .map(flatten_single_batch)
            .collect::<Vec<_>>();
        let fields = [
            &[0x07],
            self.span
                .as_ref()
                .map(flatten_span_batch)
                .unwrap_or_default()
                .digest()
                .as_bytes(),
            buffer.digest().as_bytes(),
            self.prev.digest().as_bytes(),
        ]
        .concat();
        *SHA2::hash_bytes(fields.as_slice())
    }
}

impl Digestible for CachedChannelReader {
    fn digest(&self) -> Digest {
        let fields = [
            &[0x06],
            self.next_batch
                .as_ref()
                .map(|v| {
                    [
                        v.data.digest().as_bytes(),
                        v.decompressed.as_slice(),
                        (v.cursor as u64).to_be_bytes().as_slice(),
                        (v.max_rlp_bytes_per_channel as u64)
                            .to_be_bytes()
                            .as_slice(),
                        &[v.brotli_used as u8],
                    ]
                    .concat()
                })
                .unwrap_or_default()
                .as_slice(),
            self.prev.digest().as_bytes(),
        ]
        .concat();
        *SHA2::hash_bytes(fields.as_slice())
    }
}

impl Digestible for CachedChannelProvider {
    fn digest(&self) -> Digest {
        match self {
            CachedChannelProvider::None => Digest::default(),
            CachedChannelProvider::FrameQueue(fq) => fq.digest(),
            CachedChannelProvider::ChannelBank(cb) => cb.digest(),
            CachedChannelProvider::ChannelAssembler(ca) => ca.digest(),
        }
    }
}

impl Digestible for CachedChannelBank {
    fn digest(&self) -> Digest {
        let channels = self
            .channels
            .iter()
            .map(|(_, channel)| flatten_channel(channel))
            .collect::<Vec<_>>();
        let fields = [
            &[0x05],
            (channels.len() as u64).to_be_bytes().as_slice(),
            channels.concat().as_slice(),
            (self.channel_queue.len() as u64).to_be_bytes().as_slice(),
            self.channel_queue.concat().as_slice(),
            self.prev.digest().as_bytes(),
        ]
        .concat();
        *SHA2::hash_bytes(fields.as_slice())
    }
}

impl Digestible for CachedChannelAssembler {
    fn digest(&self) -> Digest {
        let fields = [
            &[0x04],
            self.channel
                .as_ref()
                .map(flatten_channel)
                .unwrap_or_default()
                .as_slice(),
            self.prev.digest().as_bytes(),
        ]
        .concat();
        *SHA2::hash_bytes(fields.as_slice())
    }
}

impl Digestible for CachedFrameQueue {
    fn digest(&self) -> Digest {
        let queue_frames = self.queue.iter().map(flatten_frame).collect::<Vec<_>>();
        let fields = [
            &[0x03],
            (queue_frames.len() as u64).to_be_bytes().as_slice(),
            queue_frames.digest().as_bytes(),
            self.prev.digest().as_bytes(),
        ]
        .concat();
        *SHA2::hash_bytes(fields.as_slice())
    }
}

impl Digestible for CachedL1Retrieval {
    fn digest(&self) -> Digest {
        let next_bytes = self
            .next
            .as_ref()
            .map(flatten_block_info)
            .unwrap_or(vec![0xFF; 80]);
        let fields = [
            &[0x02],
            next_bytes.as_slice(),
            self.prev.digest().as_bytes(),
        ]
        .concat();
        *SHA2::hash_bytes(fields.as_slice())
    }
}

impl Digestible for CachedPollingTraversal {
    fn digest(&self) -> Digest {
        let block_bytes = self
            .block
            .as_ref()
            .map(flatten_block_info)
            .unwrap_or(vec![0xFF; 80]);

        let fields = [
            &[0x01],
            block_bytes.as_slice(),
            &[self.done as u8],
            genesis_system_config_hash(&self.system_config).as_slice(),
        ]
        .concat();

        *SHA2::hash_bytes(fields.as_slice())
    }
}

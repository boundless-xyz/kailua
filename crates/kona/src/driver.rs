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

use crate::rkyv::driver::{
    sorted_by_key, BatchReaderRkyv, BatchWithInclusionBlockRkyv, BlockInfoRkyv, ChannelRkyv,
    FrameRkyv, HeadArtifactsRkyv, IdChannelRkyv, OpAttributesWithParentRkyv, PipelineCursorRkyv,
    SingleBatchRkyv, SpanBatchRkyv, SystemConfigRkyv,
};
use alloy_eips::eip4895::Withdrawal;
use alloy_eips::Typed2718;
use alloy_primitives::Bytes;
use kona_derive::attributes::StatefulAttributesBuilder;
use kona_derive::pipeline::{
    AttributesQueueStage, BatchProviderStage, BatchStreamStage, ChannelProviderStage,
    ChannelReaderStage, DerivationPipeline, FrameQueueStage, L1RetrievalStage,
};
use kona_derive::prelude::{
    BatchQueue, BatchValidator, ChainProvider, ChannelAssembler, ChannelBank,
    DataAvailabilityProvider, L1Traversal, L2ChainProvider,
};
use kona_driver::{Driver, Executor, PipelineCursor};
use kona_executor::BlockBuildingOutcome;
use kona_genesis::{RollupConfig, SystemConfig};
use kona_preimage::CommsClient;
use kona_proof::l1::{OraclePipeline, ProviderDerivationPipeline};
use kona_proof::FlushableCache;
use kona_protocol::{
    Batch, BatchReader, BatchWithInclusionBlock, BlockInfo, Channel, ChannelId, Frame, L2BlockInfo,
    OpAttributesWithParent, SingleBatch, SpanBatch, SpanBatchElement, SpanBatchTransactions,
};
use risc0_zkvm::sha::Digestible;
use risc0_zkvm::sha::{Impl as SHA2, Sha256};
use risc0_zkvm::Digest;
use spin::RwLock;
use std::fmt::Debug;
use std::sync::Arc;

pub type KonaDriver<E, O, L1, L2, DA> =
    Driver<E, OraclePipeline<O, L1, L2, DA>, ProviderDerivationPipeline<L1, L2, DA>>;

pub fn opt_bytes<const N: usize>(data: Option<[u8; N]>) -> Vec<u8> {
    let Some(data) = data else {
        return vec![0xFF; N + 1];
    };
    let mut res = vec![0x00; N + 1];
    (&mut res[1..]).copy_from_slice(&data);
    res
}

#[derive(Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CachedDriver {
    /// Cursor to keep track of the L2 tip
    #[rkyv(with = PipelineCursorRkyv)]
    pub cursor: PipelineCursor,
    /// The safe head's execution artifacts + Transactions
    #[rkyv(with = rkyv::with::Map<HeadArtifactsRkyv>)]
    pub safe_head_artifacts: Option<(BlockBuildingOutcome, Vec<Bytes>)>,
    /// A pipeline abstraction.
    pub pipeline: CachedDerivationPipeline,
}

pub fn flatten_pipeline_cursor(pipeline_cursor: &PipelineCursor) -> Vec<u8> {
    [
        pipeline_cursor.capacity.to_be_bytes().as_slice(),
        pipeline_cursor.channel_timeout.to_be_bytes().as_slice(),
        flatten_block_info(&pipeline_cursor.origin).as_slice(),
        pipeline_cursor.origins.len().to_be_bytes().as_slice(),
        pipeline_cursor
            .origins
            .iter()
            .map(|o| o.to_be_bytes())
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
        pipeline_cursor.origin_infos.len().to_be_bytes().as_slice(),
        sorted_by_key(
            pipeline_cursor
                .origin_infos
                .clone()
                .iter()
                .collect::<Vec<_>>(),
        )
        .iter()
        .map(|(k, v)| [k.to_be_bytes().as_slice(), flatten_block_info(v).as_slice()].concat())
        .collect::<Vec<_>>()
        .concat()
        .as_slice(),
        pipeline_cursor.tips.len().to_be_bytes().as_slice(),
        pipeline_cursor
            .tips
            .iter()
            .map(|(k, v)| {
                [
                    k.to_be_bytes().as_slice(),
                    flatten_l2_block_info(&v.l2_safe_head).as_slice(),
                    v.l2_safe_head_header.hash().as_slice(),
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
        artifacts.0.header.hash().as_slice(),
        artifacts
            .0
            .execution_result
            .receipts
            .len()
            .to_be_bytes()
            .as_slice(),
        artifacts
            .0
            .execution_result
            .receipts
            .iter()
            .map(alloy_rlp::encode)
            .map(flatten_bytes)
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
        artifacts
            .0
            .execution_result
            .requests
            .len()
            .to_be_bytes()
            .as_slice(),
        artifacts
            .0
            .execution_result
            .requests
            .iter()
            .map(flatten_bytes)
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
        artifacts
            .0
            .execution_result
            .gas_used
            .to_be_bytes()
            .as_slice(),
        artifacts.1.len().to_be_bytes().as_slice(),
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

impl CachedDriver {
    #[allow(clippy::too_many_arguments)]
    pub fn uncache<E, O, L1, L2, DA>(
        self,
        executor: E,
        cfg: Arc<RollupConfig>,
        sync_start: Arc<RwLock<PipelineCursor>>,
        caching_oracle: Arc<O>,
        da_provider: DA,
        l1_chain_provider: L1,
        l2_chain_provider: L2,
    ) -> KonaDriver<E, O, L1, L2, DA>
    where
        E: Executor + Send + Sync + Debug,
        O: CommsClient + FlushableCache + Send + Sync + Debug,
        L1: ChainProvider + Send + Sync + Debug + Clone,
        L2: L2ChainProvider + Send + Sync + Debug + Clone,
        DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
    {
        // update sync_start cursor to cached value
        *sync_start.write() = self.cursor;
        // uncache oracle pipeline
        let pipeline = OraclePipeline {
            pipeline: self.pipeline.uncache(
                cfg.clone(),
                da_provider,
                l1_chain_provider,
                l2_chain_provider,
            ),
            caching_oracle: caching_oracle.clone(),
        };
        // Construct driver with pipeline
        let mut driver = Driver::new(sync_start, executor, pipeline);
        // Update safe head artifacts
        driver.safe_head_artifacts = self.safe_head_artifacts;
        // Return final driver
        driver
    }
}

impl<E, O, L1, L2, DA> From<KonaDriver<E, O, L1, L2, DA>> for CachedDriver
where
    E: Executor + Send + Sync + Debug,
    O: CommsClient + FlushableCache + Send + Sync + Debug,
    L1: ChainProvider + Send + Sync + Debug + Clone,
    L2: L2ChainProvider + Send + Sync + Debug + Clone,
    DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
{
    fn from(value: KonaDriver<E, O, L1, L2, DA>) -> Self {
        Self {
            cursor: value.cursor.read().clone(),
            safe_head_artifacts: value.safe_head_artifacts,
            pipeline: CachedDerivationPipeline::from(value.pipeline.pipeline),
        }
    }
}

#[derive(Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CachedDerivationPipeline {
    /// A list of prepared [OpAttributesWithParent] to be used by the derivation pipeline
    /// consumer.
    #[rkyv(with = rkyv::with::Map<OpAttributesWithParentRkyv>)]
    pub prepared: Vec<OpAttributesWithParent>,
    /// A handle to the next attributes.
    pub attributes: CachedAttributesQueueStage,
}

pub fn flatten_op_attrib_with_parent(op_attrib_with_parent: &OpAttributesWithParent) -> Vec<u8> {
    [
        op_attrib_with_parent
            .inner
            .payload_attributes
            .timestamp
            .to_be_bytes()
            .as_slice(),
        op_attrib_with_parent
            .inner
            .payload_attributes
            .prev_randao
            .as_slice(),
        op_attrib_with_parent
            .inner
            .payload_attributes
            .suggested_fee_recipient
            .as_slice(),
        op_attrib_with_parent
            .inner
            .payload_attributes
            .withdrawals
            .as_ref()
            .map(|v| v.len())
            .unwrap_or_default()
            .to_be_bytes()
            .as_slice(),
        op_attrib_with_parent
            .inner
            .payload_attributes
            .withdrawals
            .as_ref()
            .map(|v| v.iter().map(flatten_withdrawal).collect::<Vec<_>>())
            .unwrap_or_default()
            .concat()
            .as_slice(),
        op_attrib_with_parent
            .inner
            .payload_attributes
            .parent_beacon_block_root
            .unwrap_or_default()
            .as_slice(),
        op_attrib_with_parent
            .inner
            .transactions
            .as_ref()
            .map(|v| v.len())
            .unwrap_or_default()
            .to_be_bytes()
            .as_slice(),
        op_attrib_with_parent
            .inner
            .transactions
            .as_ref()
            .map(|v| v.iter().map(flatten_bytes).collect::<Vec<_>>())
            .unwrap_or_default()
            .concat()
            .as_slice(),
        opt_bytes(op_attrib_with_parent.inner.no_tx_pool.map(|v| [v as u8])).as_slice(),
        opt_bytes(
            op_attrib_with_parent
                .inner
                .gas_limit
                .map(|v| v.to_be_bytes()),
        )
        .as_slice(),
        opt_bytes(op_attrib_with_parent.inner.eip_1559_params.map(|v| v.0)).as_slice(),
        flatten_l2_block_info(&op_attrib_with_parent.parent).as_slice(),
        flatten_block_info(&op_attrib_with_parent.l1_origin).as_slice(),
        &[op_attrib_with_parent.is_last_in_span as u8],
    ]
    .concat()
}

pub fn flatten_withdrawal(withdrawal: &Withdrawal) -> Vec<u8> {
    [
        withdrawal.index.to_le_bytes().as_slice(),
        withdrawal.validator_index.to_le_bytes().as_slice(),
        withdrawal.address.as_slice(),
        withdrawal.amount.to_le_bytes().as_slice(),
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

impl CachedDerivationPipeline {
    pub fn uncache<L1, L2, DA>(
        self,
        cfg: Arc<RollupConfig>,
        da_provider: DA,
        l1_chain_provider: L1,
        l2_chain_provider: L2,
    ) -> ProviderDerivationPipeline<L1, L2, DA>
    where
        L1: ChainProvider + Send + Sync + Debug + Clone,
        L2: L2ChainProvider + Send + Sync + Debug + Clone,
        DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
    {
        DerivationPipeline {
            attributes: self.attributes.uncache(
                cfg.clone(),
                da_provider,
                l1_chain_provider,
                l2_chain_provider.clone(),
            ),
            prepared: self.prepared.into(),
            rollup_config: cfg,
            l2_chain_provider,
        }
    }
}

impl<DA, L1, L2> From<ProviderDerivationPipeline<L1, L2, DA>> for CachedDerivationPipeline
where
    L1: ChainProvider + Send + Sync + Debug + Clone,
    L2: L2ChainProvider + Send + Sync + Debug + Clone,
    DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
{
    fn from(value: ProviderDerivationPipeline<L1, L2, DA>) -> Self {
        Self {
            prepared: value.prepared.into(),
            attributes: CachedAttributesQueueStage::from(value.attributes),
        }
    }
}

#[derive(Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CachedAttributesQueueStage {
    /// Whether the current batch is the last in its span.
    pub is_last_in_span: bool,
    /// The current batch being processed.
    #[rkyv(with = rkyv::with::Map<SingleBatchRkyv>)]
    pub batch: Option<SingleBatch>,
    /// The previous stage of the derivation pipeline.
    pub prev: CachedBatchProvider,
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

impl CachedAttributesQueueStage {
    pub fn uncache<L1, L2, DA>(
        self,
        cfg: Arc<RollupConfig>,
        da_provider: DA,
        l1_chain_provider: L1,
        l2_chain_provider: L2,
    ) -> AttributesQueueStage<DA, L1, L2, StatefulAttributesBuilder<L1, L2>>
    where
        L1: ChainProvider + Send + Sync + Debug + Clone,
        L2: L2ChainProvider + Send + Sync + Debug + Clone,
        DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
    {
        AttributesQueueStage {
            cfg: cfg.clone(),
            prev: self.prev.uncache(
                cfg.clone(),
                da_provider,
                l1_chain_provider.clone(),
                l2_chain_provider.clone(),
            ),
            is_last_in_span: self.is_last_in_span,
            batch: self.batch,
            builder: StatefulAttributesBuilder::new(cfg, l2_chain_provider, l1_chain_provider),
        }
    }
}

impl<DA, L1, L2> From<AttributesQueueStage<DA, L1, L2, StatefulAttributesBuilder<L1, L2>>>
    for CachedAttributesQueueStage
where
    L1: ChainProvider + Send + Sync + Debug + Clone,
    L2: L2ChainProvider + Send + Sync + Debug + Clone,
    DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
{
    fn from(value: AttributesQueueStage<DA, L1, L2, StatefulAttributesBuilder<L1, L2>>) -> Self {
        Self {
            is_last_in_span: value.is_last_in_span,
            batch: value.batch,
            prev: CachedBatchProvider::from(value.prev),
        }
    }
}

#[derive(Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum CachedBatchProvider {
    None,
    BatchStream(CachedBatchStream),
    BatchQueue(CachedBatchQueue),
    BatchValidator(CachedBatchValidator),
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

impl CachedBatchProvider {
    pub fn uncache<L1, L2, DA>(
        self,
        cfg: Arc<RollupConfig>,
        da_provider: DA,
        l1_chain_provider: L1,
        l2_chain_provider: L2,
    ) -> BatchProviderStage<DA, L1, L2>
    where
        L1: ChainProvider + Send + Sync + Debug + Clone,
        L2: L2ChainProvider + Send + Sync + Debug + Clone,
        DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
    {
        match self {
            CachedBatchProvider::None => BatchProviderStage {
                cfg,
                provider: l2_chain_provider,
                prev: None,
                batch_queue: None,
                batch_validator: None,
            },
            CachedBatchProvider::BatchStream(batch_stream) => BatchProviderStage {
                cfg: cfg.clone(),
                provider: l2_chain_provider.clone(),
                prev: Some(batch_stream.uncache(
                    cfg,
                    da_provider,
                    l1_chain_provider,
                    l2_chain_provider,
                )),
                batch_queue: None,
                batch_validator: None,
            },
            CachedBatchProvider::BatchQueue(batch_queue) => BatchProviderStage {
                cfg: cfg.clone(),
                provider: l2_chain_provider.clone(),
                prev: None,
                batch_queue: Some(batch_queue.uncache(
                    cfg,
                    da_provider,
                    l1_chain_provider,
                    l2_chain_provider,
                )),
                batch_validator: None,
            },
            CachedBatchProvider::BatchValidator(batch_provider) => BatchProviderStage {
                cfg: cfg.clone(),
                provider: l2_chain_provider.clone(),
                prev: None,
                batch_queue: None,
                batch_validator: Some(batch_provider.uncache(
                    cfg,
                    da_provider,
                    l1_chain_provider,
                    l2_chain_provider,
                )),
            },
        }
    }
}

impl<DA, L1, L2> From<BatchProviderStage<DA, L1, L2>> for CachedBatchProvider
where
    L1: ChainProvider + Send + Sync + Debug + Clone,
    L2: L2ChainProvider + Send + Sync + Debug + Clone,
    DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
{
    fn from(value: BatchProviderStage<DA, L1, L2>) -> Self {
        match (value.prev, value.batch_queue, value.batch_validator) {
            (None, None, None) => CachedBatchProvider::None,
            (Some(batch_stream), None, None) => {
                CachedBatchProvider::BatchStream(CachedBatchStream::from(batch_stream))
            }
            (None, Some(batch_queue), None) => {
                CachedBatchProvider::BatchQueue(CachedBatchQueue::from(batch_queue))
            }
            (None, None, Some(batch_validator)) => {
                CachedBatchProvider::BatchValidator(CachedBatchValidator::from(batch_validator))
            }
            _ => unreachable!("More than one optional field set in BatchProviderStage."),
        }
    }
}

#[derive(Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CachedBatchQueue {
    /// The l1 block ref
    #[rkyv(with = rkyv::with::Map<BlockInfoRkyv>)]
    pub origin: Option<BlockInfo>,
    /// A consecutive, time-centric window of L1 Blocks.
    /// Every L1 origin of unsafe L2 Blocks must be included in this list.
    /// If every L2 Block corresponding to a single L1 Block becomes safe,
    /// the block is popped from this list.
    /// If new L2 Block's L1 origin is not included in this list, fetch and
    /// push it to the list.
    #[rkyv(with = rkyv::with::Map<BlockInfoRkyv>)]
    pub l1_blocks: Vec<BlockInfo>,
    /// A set of batches in order from when we've seen them.
    #[rkyv(with = rkyv::with::Map<BatchWithInclusionBlockRkyv>)]
    pub batches: Vec<BatchWithInclusionBlock>,
    /// A set of cached [SingleBatch]es derived from [SpanBatch]es.
    #[rkyv(with = rkyv::with::Map<SingleBatchRkyv>)]
    pub next_spans: Vec<SingleBatch>,
    /// The previous stage of the derivation pipeline.
    pub prev: CachedBatchStream,
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

impl CachedBatchQueue {
    pub fn uncache<L1, L2, DA>(
        self,
        cfg: Arc<RollupConfig>,
        da_provider: DA,
        l1_chain_provider: L1,
        l2_chain_provider: L2,
    ) -> BatchQueue<BatchStreamStage<DA, L1, L2>, L2>
    where
        L1: ChainProvider + Send + Sync + Debug + Clone,
        L2: L2ChainProvider + Send + Sync + Debug + Clone,
        DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
    {
        BatchQueue {
            cfg: cfg.clone(),
            prev: self.prev.uncache(
                cfg,
                da_provider,
                l1_chain_provider,
                l2_chain_provider.clone(),
            ),
            origin: self.origin,
            l1_blocks: self.l1_blocks,
            batches: self.batches,
            next_spans: self.next_spans,
            fetcher: l2_chain_provider,
        }
    }
}

impl<DA, L1, L2> From<BatchQueue<BatchStreamStage<DA, L1, L2>, L2>> for CachedBatchQueue
where
    L1: ChainProvider + Send + Sync + Debug + Clone,
    L2: L2ChainProvider + Send + Sync + Debug + Clone,
    DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
{
    fn from(value: BatchQueue<BatchStreamStage<DA, L1, L2>, L2>) -> Self {
        Self {
            origin: value.origin,
            l1_blocks: value.l1_blocks,
            batches: value.batches,
            next_spans: value.next_spans,
            prev: CachedBatchStream::from(value.prev),
        }
    }
}

#[derive(Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CachedBatchValidator {
    /// The L1 origin of the batch sequencer.
    #[rkyv(with = rkyv::with::Map<BlockInfoRkyv>)]
    pub origin: Option<BlockInfo>,
    /// A consecutive, time-centric window of L1 Blocks.
    /// Every L1 origin of unsafe L2 Blocks must be included in this list.
    /// If every L2 Block corresponding to a single L1 Block becomes safe,
    /// the block is popped from this list.
    /// If new L2 Block's L1 origin is not included in this list, fetch and
    /// push it to the list.
    #[rkyv(with = rkyv::with::Map<BlockInfoRkyv>)]
    pub l1_blocks: Vec<BlockInfo>,
    /// The previous stage of the derivation pipeline.
    pub prev: CachedBatchStream,
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

impl CachedBatchValidator {
    pub fn uncache<L1, L2, DA>(
        self,
        cfg: Arc<RollupConfig>,
        da_provider: DA,
        l1_chain_provider: L1,
        l2_chain_provider: L2,
    ) -> BatchValidator<BatchStreamStage<DA, L1, L2>>
    where
        L1: ChainProvider + Send + Sync + Debug + Clone,
        L2: L2ChainProvider + Send + Sync + Debug + Clone,
        DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
    {
        BatchValidator {
            cfg: cfg.clone(),
            prev: self
                .prev
                .uncache(cfg, da_provider, l1_chain_provider, l2_chain_provider),
            origin: self.origin,
            l1_blocks: self.l1_blocks,
        }
    }
}

impl<DA, L1, L2> From<BatchValidator<BatchStreamStage<DA, L1, L2>>> for CachedBatchValidator
where
    L1: ChainProvider + Send + Sync + Debug + Clone,
    L2: L2ChainProvider + Send + Sync + Debug + Clone,
    DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
{
    fn from(value: BatchValidator<BatchStreamStage<DA, L1, L2>>) -> Self {
        Self {
            origin: value.origin,
            l1_blocks: value.l1_blocks,
            prev: CachedBatchStream::from(value.prev),
        }
    }
}

#[derive(Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CachedBatchStream {
    /// There can only be a single staged span batch.
    #[rkyv(with = rkyv::with::Map<SpanBatchRkyv>)]
    pub span: Option<SpanBatch>,
    /// A buffer of single batches derived from the [SpanBatch].
    #[rkyv(with = rkyv::with::Map<SingleBatchRkyv>)]
    pub buffer: Vec<SingleBatch>,
    /// The previous stage in the derivation pipeline.
    pub prev: CachedChannelReader,
}

pub fn flatten_span_batch(span_batch: &SpanBatch) -> Vec<u8> {
    [
        span_batch.parent_check.as_slice(),
        span_batch.l1_origin_check.as_slice(),
        span_batch.genesis_timestamp.to_be_bytes().as_slice(),
        span_batch.chain_id.to_be_bytes().as_slice(),
        span_batch.batches.len().to_be_bytes().as_slice(),
        span_batch
            .batches
            .iter()
            .map(flatten_span_batch_element)
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
        flatten_bytes(span_batch.origin_bits.as_ref()).as_slice(),
        span_batch.block_tx_counts.len().to_be_bytes().as_slice(),
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
        span_batch_transactions
            .tx_sigs
            .len()
            .to_be_bytes()
            .as_slice(),
        span_batch_transactions
            .tx_sigs
            .iter()
            .map(|s| s.as_bytes())
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
        span_batch_transactions
            .tx_nonces
            .len()
            .to_be_bytes()
            .as_slice(),
        span_batch_transactions
            .tx_nonces
            .iter()
            .map(|v| v.to_be_bytes())
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
        span_batch_transactions
            .tx_gases
            .len()
            .to_be_bytes()
            .as_slice(),
        span_batch_transactions
            .tx_gases
            .iter()
            .map(|v| v.to_be_bytes())
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
        span_batch_transactions
            .tx_tos
            .len()
            .to_be_bytes()
            .as_slice(),
        span_batch_transactions
            .tx_tos
            .iter()
            .map(|a| *a.0)
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
        span_batch_transactions
            .tx_datas
            .len()
            .to_be_bytes()
            .as_slice(),
        span_batch_transactions
            .tx_datas
            .iter()
            .map(flatten_bytes)
            .collect::<Vec<_>>()
            .concat()
            .as_slice(),
        flatten_bytes(span_batch_transactions.protected_bits.as_ref()).as_slice(),
        span_batch_transactions
            .tx_types
            .len()
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
    [bytes.len().to_be_bytes().as_slice(), bytes].concat()
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

impl CachedBatchStream {
    pub fn uncache<L1, L2, DA>(
        self,
        cfg: Arc<RollupConfig>,
        da_provider: DA,
        l1_chain_provider: L1,
        l2_chain_provider: L2,
    ) -> BatchStreamStage<DA, L1, L2>
    where
        L1: ChainProvider + Send + Sync + Debug + Clone,
        L2: L2ChainProvider + Send + Sync + Debug + Clone,
        DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
    {
        BatchStreamStage {
            prev: self
                .prev
                .uncache(cfg.clone(), da_provider, l1_chain_provider),
            span: self.span,
            buffer: self.buffer.into(),
            config: cfg,
            fetcher: l2_chain_provider,
        }
    }
}

impl<DA, L1, L2> From<BatchStreamStage<DA, L1, L2>> for CachedBatchStream
where
    L1: ChainProvider + Send + Sync + Debug + Clone,
    L2: L2ChainProvider + Send + Sync + Debug + Clone,
    DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
{
    fn from(value: BatchStreamStage<DA, L1, L2>) -> Self {
        Self {
            span: value.span,
            buffer: value.buffer.into(),
            prev: CachedChannelReader::from(value.prev),
        }
    }
}

#[derive(Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CachedChannelReader {
    /// The batch reader.
    #[rkyv(with = rkyv::with::Map<BatchReaderRkyv>)]
    pub next_batch: Option<BatchReader>,
    /// The previous stage of the derivation pipeline.
    pub prev: CachedChannelProvider,
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
                        v.cursor.to_be_bytes().as_slice(),
                        v.max_rlp_bytes_per_channel.to_be_bytes().as_slice(),
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

impl CachedChannelReader {
    pub fn uncache<L1, DA>(
        self,
        cfg: Arc<RollupConfig>,
        da_provider: DA,
        l1_chain_provider: L1,
    ) -> ChannelReaderStage<DA, L1>
    where
        L1: ChainProvider + Send + Sync + Debug + Clone,
        DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
    {
        ChannelReaderStage {
            prev: self
                .prev
                .uncache(cfg.clone(), da_provider, l1_chain_provider),
            next_batch: self.next_batch,
            cfg,
        }
    }
}

impl<DA, L1> From<ChannelReaderStage<DA, L1>> for CachedChannelReader
where
    L1: ChainProvider + Send + Sync + Debug + Clone,
    DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
{
    fn from(value: ChannelReaderStage<DA, L1>) -> Self {
        Self {
            next_batch: value.next_batch,
            prev: CachedChannelProvider::from(value.prev),
        }
    }
}

#[derive(Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum CachedChannelProvider {
    None,
    FrameQueue(CachedFrameQueue),
    ChannelBank(CachedChannelBank),
    ChannelAssembler(CachedChannelAssembler),
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

impl CachedChannelProvider {
    pub fn uncache<L1, DA>(
        self,
        cfg: Arc<RollupConfig>,
        da_provider: DA,
        l1_chain_provider: L1,
    ) -> ChannelProviderStage<DA, L1>
    where
        L1: ChainProvider + Send + Sync + Debug + Clone,
        DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
    {
        match self {
            CachedChannelProvider::None => ChannelProviderStage {
                cfg,
                prev: None,
                channel_bank: None,
                channel_assembler: None,
            },
            CachedChannelProvider::FrameQueue(frame_queue) => ChannelProviderStage {
                cfg: cfg.clone(),
                prev: Some(frame_queue.uncache(cfg, da_provider, l1_chain_provider)),
                channel_bank: None,
                channel_assembler: None,
            },
            CachedChannelProvider::ChannelBank(channel_bank) => ChannelProviderStage {
                cfg: cfg.clone(),
                prev: None,
                channel_bank: Some(channel_bank.uncache(cfg, da_provider, l1_chain_provider)),
                channel_assembler: None,
            },
            CachedChannelProvider::ChannelAssembler(channel_assembler) => ChannelProviderStage {
                cfg: cfg.clone(),
                prev: None,
                channel_bank: None,
                channel_assembler: Some(channel_assembler.uncache(
                    cfg,
                    da_provider,
                    l1_chain_provider,
                )),
            },
        }
    }
}

impl<DA, L1> From<ChannelProviderStage<DA, L1>> for CachedChannelProvider
where
    L1: ChainProvider + Send + Sync + Debug + Clone,
    DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
{
    fn from(value: ChannelProviderStage<DA, L1>) -> Self {
        match (value.prev, value.channel_bank, value.channel_assembler) {
            (None, None, None) => CachedChannelProvider::None,
            (Some(frame_queue), None, None) => {
                CachedChannelProvider::FrameQueue(CachedFrameQueue::from(frame_queue))
            }
            (None, Some(channel_bank), None) => {
                CachedChannelProvider::ChannelBank(CachedChannelBank::from(channel_bank))
            }
            (None, None, Some(channel_assembler)) => CachedChannelProvider::ChannelAssembler(
                CachedChannelAssembler::from(channel_assembler),
            ),
            _ => unreachable!("More than one optional value set in ChannelProvider."),
        }
    }
}

#[derive(Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CachedChannelBank {
    /// Map of channels by ID.
    #[rkyv(with = rkyv::with::Map<IdChannelRkyv>)]
    pub channels: Vec<(ChannelId, Channel)>,
    /// Channels in FIFO order.
    pub channel_queue: Vec<ChannelId>,
    /// The previous stage of the derivation pipeline.
    pub prev: CachedFrameQueue,
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
            channels.concat().as_slice(),
            self.channel_queue.concat().as_slice(),
            self.prev.digest().as_bytes(),
        ]
        .concat();
        *SHA2::hash_bytes(fields.as_slice())
    }
}

impl CachedChannelBank {
    pub fn uncache<L1, DA>(
        self,
        cfg: Arc<RollupConfig>,
        da_provider: DA,
        l1_chain_provider: L1,
    ) -> ChannelBank<FrameQueueStage<DA, L1>>
    where
        L1: ChainProvider + Send + Sync + Debug + Clone,
        DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
    {
        ChannelBank {
            cfg: cfg.clone(),
            channels: self.channels.into_iter().collect(),
            channel_queue: self.channel_queue.into(),
            prev: self.prev.uncache(cfg, da_provider, l1_chain_provider),
        }
    }
}

impl<DA, L1> From<ChannelBank<FrameQueueStage<DA, L1>>> for CachedChannelBank
where
    L1: ChainProvider + Send + Sync + Debug + Clone,
    DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
{
    fn from(value: ChannelBank<FrameQueueStage<DA, L1>>) -> Self {
        Self {
            channels: sorted_by_key(value.channels.into_iter().collect()),
            channel_queue: value.channel_queue.into(),
            prev: CachedFrameQueue::from(value.prev),
        }
    }
}

#[derive(Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CachedChannelAssembler {
    /// The current [Channel] being assembled.
    #[rkyv(with = rkyv::with::Map<ChannelRkyv>)]
    pub channel: Option<Channel>,
    /// The previous stage of the derivation pipeline.
    pub prev: CachedFrameQueue,
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
        channel.estimated_size.to_be_bytes().as_slice(),
        &[channel.closed as u8],
        channel.highest_frame_number.to_be_bytes().as_slice(),
        channel.last_frame_number.to_be_bytes().as_slice(),
        inputs.as_slice(),
        flatten_block_info(&channel.highest_l1_inclusion_block).as_slice(),
    ]
    .concat()
}

impl Digestible for CachedChannelAssembler {
    fn digest(&self) -> Digest {
        let fields = [
            &[0x04],
            self.channel
                .as_ref()
                .map(flatten_channel)
                .unwrap_or(vec![])
                .as_slice(),
            self.prev.digest().as_bytes(),
        ]
        .concat();
        *SHA2::hash_bytes(fields.as_slice())
    }
}

impl CachedChannelAssembler {
    pub fn uncache<L1, DA>(
        self,
        cfg: Arc<RollupConfig>,
        da_provider: DA,
        l1_chain_provider: L1,
    ) -> ChannelAssembler<FrameQueueStage<DA, L1>>
    where
        L1: ChainProvider + Send + Sync + Debug + Clone,
        DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
    {
        ChannelAssembler {
            cfg: cfg.clone(),
            prev: self.prev.uncache(cfg, da_provider, l1_chain_provider),
            channel: self.channel,
        }
    }
}

impl<DA, L1> From<ChannelAssembler<FrameQueueStage<DA, L1>>> for CachedChannelAssembler
where
    L1: ChainProvider + Send + Sync + Debug + Clone,
    DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
{
    fn from(value: ChannelAssembler<FrameQueueStage<DA, L1>>) -> Self {
        Self {
            channel: value.channel,
            prev: CachedFrameQueue::from(value.prev),
        }
    }
}

#[derive(Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CachedFrameQueue {
    /// The current frame queue.
    #[rkyv(with = rkyv::with::Map<FrameRkyv>)]
    pub queue: Vec<Frame>,
    /// The previous stage in the pipeline.
    pub prev: CachedL1Retrieval,
}

impl Digestible for CachedFrameQueue {
    fn digest(&self) -> Digest {
        let queue_frames = self.queue.iter().map(flatten_frame).collect::<Vec<_>>();
        let fields = [
            &[0x03],
            queue_frames.digest().as_bytes(),
            self.prev.digest().as_bytes(),
        ]
        .concat();
        *SHA2::hash_bytes(fields.as_slice())
    }
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

impl CachedFrameQueue {
    pub fn uncache<L1, DA>(
        self,
        cfg: Arc<RollupConfig>,
        da_provider: DA,
        l1_chain_provider: L1,
    ) -> FrameQueueStage<DA, L1>
    where
        L1: ChainProvider + Send + Sync + Debug + Clone,
        DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
    {
        FrameQueueStage {
            prev: self
                .prev
                .uncache(cfg.clone(), da_provider, l1_chain_provider),
            queue: self.queue.into(),
            rollup_config: cfg,
        }
    }
}

impl<DA, L1> From<FrameQueueStage<DA, L1>> for CachedFrameQueue
where
    L1: ChainProvider + Send + Sync + Debug + Clone,
    DA: DataAvailabilityProvider + Send + Sync + Debug + Clone,
{
    fn from(value: FrameQueueStage<DA, L1>) -> Self {
        Self {
            queue: value.queue.into(),
            prev: CachedL1Retrieval::from(value.prev),
        }
    }
}

#[derive(Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CachedL1Retrieval {
    /// The current block ref.
    #[rkyv(with = rkyv::with::Map<BlockInfoRkyv>)]
    pub next: Option<BlockInfo>,
    /// The previous stage in the pipeline.
    pub prev: CachedL1Traversal,
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

impl CachedL1Retrieval {
    pub fn uncache<L1, DA>(
        self,
        cfg: Arc<RollupConfig>,
        da_provider: DA,
        l1_chain_provider: L1,
    ) -> L1RetrievalStage<DA, L1>
    where
        DA: DataAvailabilityProvider,
        L1: ChainProvider + Send + Sync + Debug + Clone,
    {
        L1RetrievalStage {
            prev: self.prev.uncache(cfg, l1_chain_provider),
            provider: da_provider,
            next: self.next,
        }
    }
}

impl<DA, L1> From<L1RetrievalStage<DA, L1>> for CachedL1Retrieval
where
    DA: DataAvailabilityProvider,
    L1: ChainProvider + Send + Sync + Debug + Clone,
{
    fn from(value: L1RetrievalStage<DA, L1>) -> Self {
        Self {
            next: value.next,
            prev: CachedL1Traversal::from(value.prev),
        }
    }
}

#[derive(Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CachedL1Traversal {
    /// The current block in the traversal stage.
    #[rkyv(with = rkyv::with::Map<BlockInfoRkyv>)]
    pub block: Option<BlockInfo>,
    /// Signals whether or not the traversal stage is complete.
    pub done: bool,
    /// The system config.
    #[rkyv(with = SystemConfigRkyv)]
    pub system_config: SystemConfig,
}

impl Digestible for CachedL1Traversal {
    fn digest(&self) -> Digest {
        let block_bytes = self
            .block
            .as_ref()
            .map(flatten_block_info)
            .unwrap_or(vec![0xFF; 80]);
        let system_config_bytes = [
            self.system_config.batcher_address.as_slice(),
            self.system_config.overhead.to_be_bytes::<32>().as_slice(),
            self.system_config.scalar.to_be_bytes::<32>().as_slice(),
            self.system_config.gas_limit.to_be_bytes().as_slice(),
            &opt_bytes(self.system_config.base_fee_scalar.map(|v| v.to_be_bytes())),
            &opt_bytes(
                self.system_config
                    .blob_base_fee_scalar
                    .map(|v| v.to_be_bytes()),
            ),
            &opt_bytes(
                self.system_config
                    .eip1559_denominator
                    .map(|v| v.to_be_bytes()),
            ),
            &opt_bytes(
                self.system_config
                    .eip1559_elasticity
                    .map(|v| v.to_be_bytes()),
            ),
            &opt_bytes(
                self.system_config
                    .operator_fee_scalar
                    .map(|v| v.to_be_bytes()),
            ),
            &opt_bytes(
                self.system_config
                    .operator_fee_constant
                    .map(|v| v.to_be_bytes()),
            ),
        ]
        .concat();

        let fields = [
            &[0x01],
            block_bytes.as_slice(),
            &[self.done as u8],
            system_config_bytes.as_slice(),
        ]
        .concat();

        *SHA2::hash_bytes(fields.as_slice())
    }
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

impl CachedL1Traversal {
    pub fn uncache<L1>(self, cfg: Arc<RollupConfig>, l1_chain_provider: L1) -> L1Traversal<L1>
    where
        L1: ChainProvider + Send + Sync + Debug + Clone,
    {
        L1Traversal {
            block: self.block,
            data_source: l1_chain_provider,
            done: self.done,
            system_config: self.system_config,
            rollup_config: cfg,
        }
    }
}

impl<L1> From<L1Traversal<L1>> for CachedL1Traversal
where
    L1: ChainProvider + Send + Sync + Debug + Clone,
{
    fn from(value: L1Traversal<L1>) -> Self {
        Self {
            block: value.block,
            done: value.done,
            system_config: value.system_config,
        }
    }
}

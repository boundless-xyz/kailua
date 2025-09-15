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
    BatchReader, BatchWithInclusionBlock, BlockInfo, Channel, ChannelId, Frame,
    OpAttributesWithParent, SingleBatch, SpanBatch,
};
use spin::RwLock;
use std::fmt::Debug;
use std::sync::Arc;

pub type KonaDriver<E, O, L1, L2, DA> =
    Driver<E, OraclePipeline<O, L1, L2, DA>, ProviderDerivationPipeline<L1, L2, DA>>;

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CachedDerivationPipeline {
    /// A list of prepared [OpAttributesWithParent] to be used by the derivation pipeline
    /// consumer.
    #[rkyv(with = rkyv::with::Map<OpAttributesWithParentRkyv>)]
    pub prepared: Vec<OpAttributesWithParent>,
    /// A handle to the next attributes.
    pub attributes: CachedAttributesQueueStage,
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

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CachedAttributesQueueStage {
    /// Whether the current batch is the last in its span.
    pub is_last_in_span: bool,
    /// The current batch being processed.
    #[rkyv(with = rkyv::with::Map<SingleBatchRkyv>)]
    pub batch: Option<SingleBatch>,
    /// The previous stage of the derivation pipeline.
    pub prev: CachedBatchProvider,
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

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum CachedBatchProvider {
    None,
    BatchStream(CachedBatchStream),
    BatchQueue(CachedBatchQueue),
    BatchValidator(CachedBatchValidator),
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

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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

impl Clone for CachedChannelReader {
    fn clone(&self) -> Self {
        Self {
            next_batch: self.next_batch.as_ref().map(|v| BatchReader {
                data: v.data.clone(),
                decompressed: v.decompressed.clone(),
                cursor: v.cursor,
                max_rlp_bytes_per_channel: v.max_rlp_bytes_per_channel,
            }),
            prev: self.prev.clone(),
        }
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

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum CachedChannelProvider {
    None,
    FrameQueue(CachedFrameQueue),
    ChannelBank(CachedChannelBank),
    ChannelAssembler(CachedChannelAssembler),
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

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CachedChannelBank {
    /// Map of channels by ID.
    #[rkyv(with = rkyv::with::Map<IdChannelRkyv>)]
    pub channels: Vec<(ChannelId, Channel)>,
    /// Channels in FIFO order.
    pub channel_queue: Vec<ChannelId>,
    /// The previous stage of the derivation pipeline.
    pub prev: CachedFrameQueue,
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

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CachedChannelAssembler {
    /// The current [Channel] being assembled.
    #[rkyv(with = rkyv::with::Map<ChannelRkyv>)]
    pub channel: Option<Channel>,
    /// The previous stage of the derivation pipeline.
    pub prev: CachedFrameQueue,
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

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CachedFrameQueue {
    /// The current frame queue.
    #[rkyv(with = rkyv::with::Map<FrameRkyv>)]
    pub queue: Vec<Frame>,
    /// The previous stage in the pipeline.
    pub prev: CachedL1Retrieval,
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

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CachedL1Retrieval {
    /// The current block ref.
    #[rkyv(with = rkyv::with::Map<BlockInfoRkyv>)]
    pub next: Option<BlockInfo>,
    /// The previous stage in the pipeline.
    pub prev: CachedL1Traversal,
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

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod tests {
    use super::*;
    use crate::client::core::tests::test_derivation;
    use alloy_primitives::{b256, B256};
    use kona_proof::BootInfo;
    use risc0_zkvm::sha::Digestible;
    use std::sync::Mutex;

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_bob_mainnet_11791800_11791899_driver_cache() {
        let mut cached_driver: Option<CachedDriver> = None;
        let mut agreed_l2_output_root = b256!(
            "0xdf4bd3e4b13f7ed35f536129e6f853d643a2bd7f906e22090dc011928a2a02ac" // 11791799
        );
        let derivations = [
            (
                b256!("0x049070993a1aa9f42b0a66a197b71e6f466d589770292aacd40af4213f68d2de"),
                11791806,
            ),
            (
                b256!("0x3a2aeb1312c09308cf61903c682108fefa880ee411a75ee91275f27f569a2f83"),
                11791807,
            ),
            (
                b256!("0xfca53deb01a9e67b8da9308dfbbe89c0fbd4af4f695c89c71a4dc24779b95d14"),
                11791808,
            ),
            (
                b256!("0xc360c08c8d03f125cdd1644d1bf28ca5b305e9801040009f485c6a4abc378ddc"),
                11791849,
            ),
            (
                b256!("0xd65880a4aceae01adc4f4316db071188c8df9444d45c83424f13c380e2f717ce"),
                11791899,
            ),
        ];

        for (claimed_l2_output_root, claimed_l2_block_number) in derivations {
            let derivation_trace: Arc<Mutex<Option<CachedDriver>>> = Default::default();
            test_derivation(
                BootInfo {
                    l1_head: b256!(
                        "0x15e4a9386d4fab6c99378b858545fd21646e29673e80593218f356078dfbd574"
                    ),
                    agreed_l2_output_root,
                    claimed_l2_output_root,
                    claimed_l2_block_number,
                    chain_id: 60808,
                    rollup_config: Default::default(),
                },
                None,
                cached_driver,
                Some(derivation_trace.clone()),
            )
            .unwrap();
            agreed_l2_output_root = claimed_l2_output_root;
            // Verify derivation trace
            let traced_driver = derivation_trace.lock().unwrap().take().unwrap();
            let traced_driver_hash = B256::new(traced_driver.digest().into());
            let encoded_driver = rkyv::to_bytes::<rkyv::rancor::Error>(&traced_driver)
                .unwrap()
                .to_vec();
            let decoded_driver =
                rkyv::from_bytes::<CachedDriver, rkyv::rancor::Error>(&encoded_driver).unwrap();
            assert_eq!(
                traced_driver_hash,
                B256::new(decoded_driver.digest().into())
            );
            cached_driver = Some(decoded_driver);
        }
    }
}

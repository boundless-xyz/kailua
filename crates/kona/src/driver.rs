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
    use crate::boot::StitchedBootInfo;
    use crate::client::core::tests::test_derivation;
    use crate::client::core::{fetch_safe_head_hash, DASourceProvider, EthereumDataSourceProvider};
    use crate::client::stitching::tests::test_stitching_client;
    use crate::client::tests::TestOracle;
    use crate::kona::OracleL1ChainProvider;
    use crate::precondition::Precondition;
    use crate::rkyv::execution::tests::{gen_execution_outcomes, gen_header};
    use alloy_consensus::TxType;
    use alloy_eips::eip4895::Withdrawal;
    use alloy_eips::BlockNumHash;
    use alloy_op_evm::OpEvmFactory;
    use alloy_primitives::ruint::aliases::U256;
    use alloy_primitives::{b256, keccak256, Address, Sealable, Signature, B256, B64};
    use alloy_rpc_types_engine::PayloadAttributes;
    use anyhow::Context;
    use kona_driver::TipCursor;
    use kona_executor::TrieDBProvider;
    use kona_proof::executor::KonaExecutor;
    use kona_proof::l1::OracleBlobProvider;
    use kona_proof::l2::OracleL2ChainProvider;
    use kona_proof::BootInfo;
    use kona_protocol::{
        Batch, BatchValidationProvider, L2BlockInfo, SpanBatchBits, SpanBatchElement,
        SpanBatchTransactions,
    };
    use lazy_static::lazy_static;
    use op_alloy_rpc_types_engine::OpPayloadAttributes;
    use risc0_zkvm::sha::Digestible;
    use std::collections::hash_map::Entry;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Mutex;
    use std::thread::{current, ThreadId};

    pub async fn check_traced_driver(traced_driver: &CachedDriver) {
        // Test serde
        let traced_driver_hash = B256::new(traced_driver.digest().into());
        let encoded_driver = rkyv::to_bytes::<rkyv::rancor::Error>(traced_driver)
            .unwrap()
            .to_vec();
        let decoded_driver =
            rkyv::from_bytes::<CachedDriver, rkyv::rancor::Error>(&encoded_driver).unwrap();
        assert_eq!(
            traced_driver_hash,
            B256::new(decoded_driver.digest().into())
        );
        // Test caching
        let boot_info = BootInfo {
            l1_head: Default::default(),
            agreed_l2_output_root: Default::default(),
            claimed_l2_output_root: Default::default(),
            claimed_l2_block_number: 0,
            chain_id: 0,
            rollup_config: Default::default(),
        };
        let oracle = Arc::new(TestOracle::new(boot_info.clone()));
        let l1_provider = OracleL1ChainProvider::new(B256::ZERO, oracle.clone())
            .await
            .unwrap();
        let rollup_config = Arc::new(boot_info.rollup_config.clone());
        let l2_provider =
            OracleL2ChainProvider::new(B256::ZERO, rollup_config.clone(), oracle.clone());
        let da_provider = EthereumDataSourceProvider.new_from_parts(
            l1_provider.clone(),
            OracleBlobProvider::new(oracle.clone()),
            rollup_config.as_ref(),
        );
        let kona_driver = decoded_driver.uncache(
            KonaExecutor::new(
                &boot_info.rollup_config,
                l2_provider.clone(),
                l2_provider.clone(),
                OpEvmFactory::default(),
                None,
            ),
            rollup_config.clone(),
            Arc::new(RwLock::new(gen_pipeline_cursor())),
            oracle.clone(),
            da_provider,
            l1_provider,
            l2_provider,
        );
        let cached_driver = CachedDriver::from(kona_driver);
        assert_eq!(traced_driver_hash, B256::new(cached_driver.digest().into()));
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_bob_mainnet_11791800_11791899_driver_cache() {
        // kona_cli::init_tracing_subscriber(3, None::<tracing_subscriber::EnvFilter>).unwrap();

        // Load all l1 heads into list
        let mut l1_heads = vec![b256!(
            "0xe53a97cac478b7ed6846a752d552dc13011a9c1158f834fe3a3bd7d5ba5b5b63" // 21589900
        )];
        let oracle = Arc::new(TestOracle::new(BootInfo {
            l1_head: Default::default(),
            agreed_l2_output_root: Default::default(),
            claimed_l2_output_root: Default::default(),
            claimed_l2_block_number: 0,
            chain_id: 60808,
            rollup_config: Default::default(),
        }));
        let rollup_config = Arc::new(
            BootInfo::load(oracle.as_ref())
                .await
                .context("BootInfo::load")
                .unwrap()
                .rollup_config,
        );

        loop {
            let l1_head = *l1_heads.last().unwrap();
            let Ok(mut l1_provider) = OracleL1ChainProvider::new(l1_head, oracle.clone()).await
            else {
                l1_heads.pop();
                break;
            };
            let Ok(header) = l1_provider.header_by_hash(l1_head).await else {
                l1_heads.pop();
                break;
            };
            l1_heads.push(header.parent_hash);
        }
        dbg!(l1_heads.len());

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
            (
                b256!("0xf51ebc1bb7fedefa0c79040143e64e45cc4b90e8f15bc0fc66d1ea868bd6f656"),
                11795900,
            ),
        ];

        let mut cached_safe_driver: Option<CachedDriver> = None;
        let mut cached_bail_driver: Option<CachedDriver> = None;
        let mut agreed_l2_output_root = b256!(
            "0xdf4bd3e4b13f7ed35f536129e6f853d643a2bd7f906e22090dc011928a2a02ac" // 11791799
        );

        let mut stitched_preconditions = vec![];
        let mut stitched_boot_info = vec![];
        let mut i = 0;

        while i < derivations.len() {
            let (claimed_l2_output_root, claimed_l2_block_number) = &derivations[i];

            let bail_derivation_trace: Arc<Mutex<Option<CachedDriver>>> = Default::default();
            let l1_head = *l1_heads.last().unwrap();
            let bail_derivation_result = test_derivation(
                BootInfo {
                    l1_head,
                    agreed_l2_output_root,
                    claimed_l2_output_root: *claimed_l2_output_root,
                    claimed_l2_block_number: *claimed_l2_block_number,
                    chain_id: 60808,
                    rollup_config: Default::default(), // Config for BOB mainnet is in registry
                },
                None,
                cached_bail_driver.clone(),
                Some(bail_derivation_trace.clone()),
            );

            // Verify derivation trace
            let Some(traced_bail_driver) = bail_derivation_trace.lock().unwrap().take() else {
                // verify l1 head was before l1 origin
                let safe_head_hash = fetch_safe_head_hash(oracle.as_ref(), agreed_l2_output_root)
                    .await
                    .unwrap();
                let mut l2_provider = OracleL2ChainProvider::new(
                    safe_head_hash,
                    rollup_config.clone(),
                    oracle.clone(),
                );
                let safe_header = l2_provider.header_by_hash(safe_head_hash).unwrap();
                let safe_head_info = l2_provider
                    .l2_block_info_by_number(safe_header.number)
                    .await
                    .unwrap();
                let mut l1_provider = OracleL1ChainProvider::new(l1_head, oracle.clone())
                    .await
                    .unwrap();
                let l1_header = l1_provider.header_by_hash(l1_head).await.unwrap();
                assert!(safe_head_info.l1_origin.number > l1_header.number);
                l1_heads.pop();
                dbg!(l1_heads.len());
                continue;
            };
            check_traced_driver(&traced_bail_driver).await;

            // reuse bail driver
            cached_bail_driver = Some(traced_bail_driver.clone());

            // Check for insufficient data
            if let Err(err) = bail_derivation_result {
                // this derivation has failed due to insufficient l1 data
                dbg!((l1_heads.len(), err));
                // advance the l1 head
                for _ in 0..25 {
                    l1_heads.pop();
                }
                continue;
            }

            // Check for driver state equivalence
            let safe_derivation_trace: Arc<Mutex<Option<CachedDriver>>> = Default::default();
            test_derivation(
                BootInfo {
                    l1_head,
                    agreed_l2_output_root,
                    claimed_l2_output_root: *claimed_l2_output_root,
                    claimed_l2_block_number: *claimed_l2_block_number,
                    chain_id: 60808,
                    rollup_config: Default::default(), // Config for BOB mainnet is in registry
                },
                None,
                cached_safe_driver.clone(),
                Some(safe_derivation_trace.clone()),
            )
            .unwrap();
            let traced_safe_driver = safe_derivation_trace.lock().unwrap().take().unwrap();
            assert_eq!(traced_safe_driver.digest(), traced_bail_driver.digest(),);

            dbg!((i, l1_head));

            // Store precondition
            let cached_driver_digest = cached_safe_driver
                .as_ref()
                .map(|d| B256::new(d.digest().into()))
                .unwrap_or_default();
            stitched_preconditions.push(Precondition::default().derivation(
                cached_driver_digest,
                B256::new(traced_safe_driver.digest().into()),
            ));
            stitched_boot_info.push(StitchedBootInfo {
                l1_head: l1_heads[0], // todo: support l1 head stitching
                agreed_l2_output_root,
                claimed_l2_output_root: *claimed_l2_output_root,
                claimed_l2_block_number: *claimed_l2_block_number,
            });

            // Update cached safe driver
            cached_safe_driver = Some(traced_safe_driver);
            // Update agreed output
            agreed_l2_output_root = *claimed_l2_output_root;
            // Update target derivation
            i += 1;
        }

        // Backward stitch all derivations
        assert_eq!(stitched_preconditions.len(), derivations.len());
        assert_eq!(stitched_boot_info.len(), derivations.len());
        println!("stitch");
        stitched_preconditions.reverse();
        stitched_boot_info.reverse();
        let proof_journal = test_stitching_client(
            BootInfo {
                l1_head: l1_heads[0],
                agreed_l2_output_root: b256!(
                    "0xf51ebc1bb7fedefa0c79040143e64e45cc4b90e8f15bc0fc66d1ea868bd6f656"
                ),
                claimed_l2_output_root: b256!(
                    "0xf51ebc1bb7fedefa0c79040143e64e45cc4b90e8f15bc0fc66d1ea868bd6f656"
                ),
                claimed_l2_block_number: 11795900,
                chain_id: 60808,
                rollup_config: Default::default(),
            },
            None,
            vec![],
            cached_safe_driver,
            false,
            stitched_preconditions,
            stitched_boot_info,
        );
        assert_eq!(
            proof_journal.claimed_l2_output_root,
            derivations.last().unwrap().0
        );
        assert_eq!(
            proof_journal.claimed_l2_block_number,
            derivations.last().unwrap().1
        );
    }

    lazy_static! {
        static ref RNG_VAL: Arc<Mutex<HashMap<ThreadId, u64>>> = Default::default();
    }

    fn gen_b256() -> B256 {
        let data = {
            let mut rng_val = RNG_VAL.lock().unwrap();
            let val = match rng_val.entry(current().id()) {
                Entry::Occupied(mut val) => val.insert(val.get() + 1),
                Entry::Vacant(vac) => *vac.insert(0),
            };
            val.to_be_bytes()
        };
        keccak256(data.as_slice())
    }

    fn gen_u64() -> u64 {
        u64::from_be_bytes(gen_b256().0[..8].try_into().unwrap())
    }

    fn gen_usize() -> usize {
        u32::from_be_bytes(gen_b256().0[..4].try_into().unwrap()) as usize
    }

    fn gen_block_info() -> BlockInfo {
        BlockInfo {
            hash: gen_b256(),
            number: gen_u64(),
            parent_hash: gen_b256(),
            timestamp: gen_u64(),
        }
    }

    fn gen_block_num_hash() -> BlockNumHash {
        BlockNumHash {
            number: gen_u64(),
            hash: gen_b256(),
        }
    }

    fn gen_l2_block_info() -> L2BlockInfo {
        L2BlockInfo {
            block_info: gen_block_info(),
            l1_origin: gen_block_num_hash(),
            seq_num: gen_u64(),
        }
    }

    fn gen_tip_cursor() -> TipCursor {
        TipCursor {
            l2_safe_head: gen_l2_block_info(),
            l2_safe_head_header: gen_header(gen_u64()).seal_slow(),
            l2_safe_head_output_root: gen_b256(),
        }
    }

    pub fn gen_addr() -> Address {
        Address::from_slice(&gen_b256().0[..20])
    }

    pub fn gen_withdrawal() -> Withdrawal {
        Withdrawal {
            index: gen_u64(),
            validator_index: gen_u64(),
            address: gen_addr(),
            amount: gen_u64(),
        }
    }

    pub fn gen_single_batch() -> SingleBatch {
        SingleBatch {
            parent_hash: gen_b256(),
            epoch_num: gen_u64(),
            epoch_hash: gen_b256(),
            timestamp: gen_u64(),
            transactions: vec![[*gen_b256(), *gen_b256(), *gen_b256(), *gen_b256()]
                .concat()
                .into()],
        }
    }

    pub fn gen_span_batch_elem() -> SpanBatchElement {
        SpanBatchElement {
            epoch_num: gen_u64(),
            timestamp: gen_u64(),
            transactions: vec![[*gen_b256()].concat().into(), [*gen_b256()].concat().into()],
        }
    }

    pub fn gen_span_batch() -> SpanBatch {
        SpanBatch {
            parent_check: *gen_addr(),
            l1_origin_check: *gen_addr(),
            genesis_timestamp: gen_u64(),
            chain_id: gen_u64(),
            batches: vec![gen_span_batch_elem(), gen_span_batch_elem()],
            origin_bits: SpanBatchBits([*gen_b256()].concat()),
            block_tx_counts: vec![gen_u64()],
            txs: SpanBatchTransactions {
                total_block_tx_count: gen_u64(),
                contract_creation_bits: SpanBatchBits([*gen_addr()].concat()),
                tx_sigs: vec![Signature::from_raw(
                    [gen_b256().as_slice(), gen_b256().as_slice(), &[0x00]]
                        .concat()
                        .as_slice(),
                )
                .unwrap()],
                tx_nonces: vec![gen_u64()],
                tx_gases: vec![gen_u64()],
                tx_tos: vec![gen_addr()],
                tx_datas: vec![[*gen_b256()].concat()],
                protected_bits: SpanBatchBits([*gen_addr()].concat()),
                tx_types: vec![TxType::Eip1559],
                legacy_tx_count: gen_u64(),
            },
        }
    }

    pub fn gen_channel_id() -> ChannelId {
        (&gen_b256().as_slice()[..16]).try_into().unwrap()
    }

    pub fn gen_frame() -> Frame {
        Frame {
            id: gen_channel_id(),
            number: gen_u64() as u16,
            data: [*gen_b256()].concat(),
            is_last: true,
        }
    }

    pub fn gen_frame_queue() -> CachedFrameQueue {
        CachedFrameQueue {
            queue: vec![gen_frame(), gen_frame(), gen_frame()],
            prev: CachedL1Retrieval {
                next: Some(gen_block_info()),
                prev: CachedL1Traversal {
                    block: Some(gen_block_info()),
                    done: true,
                    system_config: SystemConfig {
                        batcher_address: gen_addr(),
                        overhead: U256::from_be_bytes(*gen_b256()),
                        scalar: U256::from_be_bytes(*gen_b256()),
                        gas_limit: gen_u64(),
                        base_fee_scalar: Some(gen_u64()),
                        blob_base_fee_scalar: Some(gen_u64()),
                        eip1559_denominator: Some(gen_u64() as u32),
                        eip1559_elasticity: Some(gen_u64() as u32),
                        operator_fee_scalar: Some(gen_u64() as u32),
                        operator_fee_constant: Some(gen_u64()),
                    },
                },
            },
        }
    }

    pub fn gen_channel() -> Channel {
        Channel {
            id: gen_channel_id(),
            open_block: gen_block_info(),
            estimated_size: gen_usize(),
            closed: true,
            highest_frame_number: gen_u64() as u16,
            last_frame_number: gen_u64() as u16,
            inputs: alloy_primitives::map::HashMap::from_iter(vec![
                (gen_u64() as u16, gen_frame()),
                (gen_u64() as u16, gen_frame()),
                (gen_u64() as u16, gen_frame()),
            ]),
            highest_l1_inclusion_block: gen_block_info(),
        }
    }

    pub fn gen_pipeline_cursor() -> PipelineCursor {
        PipelineCursor {
            capacity: gen_usize(),
            channel_timeout: gen_u64(),
            origin: gen_block_info(),
            origins: vec![gen_u64(), gen_u64(), gen_u64()].into(),
            origin_infos: alloy_primitives::map::HashMap::from_iter(vec![
                (gen_u64(), gen_block_info()),
                (gen_u64(), gen_block_info()),
                (gen_u64(), gen_block_info()),
                (gen_u64(), gen_block_info()),
            ]),
            tips: BTreeMap::from_iter(vec![
                (gen_u64(), gen_tip_cursor()),
                (gen_u64(), gen_tip_cursor()),
                (gen_u64(), gen_tip_cursor()),
            ]),
        }
    }

    pub async fn check_driver_batch_provider_encoding(batch_provider: CachedBatchProvider) {
        let driver = CachedDriver {
            cursor: gen_pipeline_cursor(),
            safe_head_artifacts: Some((
                gen_execution_outcomes(1).pop().unwrap(),
                vec![gen_b256().to_vec().into()],
            )),
            pipeline: CachedDerivationPipeline {
                prepared: vec![OpAttributesWithParent {
                    inner: OpPayloadAttributes {
                        payload_attributes: PayloadAttributes {
                            timestamp: gen_u64(),
                            prev_randao: gen_b256(),
                            suggested_fee_recipient: gen_addr(),
                            withdrawals: Some(vec![gen_withdrawal(), gen_withdrawal()]),
                            parent_beacon_block_root: Some(gen_b256()),
                        },
                        transactions: Some(vec![
                            gen_b256().to_vec().into(),
                            gen_b256().to_vec().into(),
                            gen_b256().to_vec().into(),
                        ]),
                        no_tx_pool: Some(true),
                        gas_limit: Some(gen_u64()),
                        eip_1559_params: Some(B64::from_slice(&gen_b256().0[..8])),
                    },
                    parent: gen_l2_block_info(),
                    l1_origin: gen_block_info(),
                    is_last_in_span: true,
                }],
                attributes: CachedAttributesQueueStage {
                    is_last_in_span: true,
                    batch: Some(gen_single_batch()),
                    prev: batch_provider,
                },
            },
        };

        check_traced_driver(&driver).await;
    }

    pub async fn check_driver_batch_stream_encoding(batch_stream: CachedBatchStream) {
        println!("BatchStream");
        check_driver_batch_provider_encoding(CachedBatchProvider::BatchStream(
            batch_stream.clone(),
        ))
        .await;
        println!("BatchQueue/Single");
        check_driver_batch_provider_encoding(CachedBatchProvider::BatchQueue(CachedBatchQueue {
            origin: Some(gen_block_info()),
            l1_blocks: vec![gen_block_info()],
            batches: vec![BatchWithInclusionBlock {
                inclusion_block: gen_block_info(),
                batch: Batch::Single(gen_single_batch()),
            }],
            next_spans: vec![gen_single_batch(), gen_single_batch(), gen_single_batch()],
            prev: batch_stream.clone(),
        }))
        .await;
        println!("BatchQueue/Span");
        check_driver_batch_provider_encoding(CachedBatchProvider::BatchQueue(CachedBatchQueue {
            origin: Some(gen_block_info()),
            l1_blocks: vec![gen_block_info()],
            batches: vec![BatchWithInclusionBlock {
                inclusion_block: gen_block_info(),
                batch: Batch::Span(gen_span_batch()),
            }],
            next_spans: vec![gen_single_batch(), gen_single_batch(), gen_single_batch()],
            prev: batch_stream.clone(),
        }))
        .await;
        println!("BatchValidator");
        check_driver_batch_provider_encoding(CachedBatchProvider::BatchValidator(
            CachedBatchValidator {
                origin: Some(gen_block_info()),
                l1_blocks: vec![gen_block_info(), gen_block_info()],
                prev: batch_stream.clone(),
            },
        ))
        .await;
    }

    pub async fn check_driver_channel_provider(channel_provider: CachedChannelProvider) {
        check_driver_batch_stream_encoding(CachedBatchStream {
            span: Some(gen_span_batch()),
            buffer: vec![gen_single_batch(), gen_single_batch(), gen_single_batch()],
            prev: CachedChannelReader {
                next_batch: Some(BatchReader {
                    data: Some([*gen_b256(), *gen_b256(), *gen_b256()].concat()),
                    decompressed: [*gen_b256(), *gen_b256(), *gen_b256()].concat(),
                    cursor: gen_usize(),
                    max_rlp_bytes_per_channel: gen_usize(),
                }),
                prev: channel_provider,
            },
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_driver_encodings() {
        println!("CachedBatchProvider");
        check_driver_batch_provider_encoding(CachedBatchProvider::None).await;
        println!("CachedChannelProvider");
        check_driver_channel_provider(CachedChannelProvider::None).await;
        println!("FrameQueue");
        check_driver_channel_provider(CachedChannelProvider::FrameQueue(gen_frame_queue())).await;
        println!("CachedChannelBank");
        check_driver_channel_provider(CachedChannelProvider::ChannelBank(CachedChannelBank {
            channels: sorted_by_key(vec![
                (gen_channel_id(), gen_channel()),
                (gen_channel_id(), gen_channel()),
                (gen_channel_id(), gen_channel()),
                (gen_channel_id(), gen_channel()),
                (gen_channel_id(), gen_channel()),
                (gen_channel_id(), gen_channel()),
            ]),
            channel_queue: vec![gen_channel_id(), gen_channel_id(), gen_channel_id()],
            prev: gen_frame_queue(),
        }))
        .await;
        println!("ChannelAssembler");
        check_driver_channel_provider(CachedChannelProvider::ChannelAssembler(
            CachedChannelAssembler {
                channel: Some(gen_channel()),
                prev: gen_frame_queue(),
            },
        ))
        .await;
    }
}

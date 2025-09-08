// Copyright 2024, 2025 RISC Zero, Inc.
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

use alloy_eips::Typed2718;
use alloy_primitives::map::HashMap;
use alloy_primitives::{Signature, U256};
use kona_genesis::SystemConfig;
use kona_protocol::{
    Batch, BatchReader, BatchWithInclusionBlock, BlockInfo, Channel, ChannelId, Frame, SingleBatch,
    SpanBatch, SpanBatchBits, SpanBatchElement, SpanBatchTransactions,
};
use rkyv::rancor::{Fallible, Source};
use rkyv::ser::{Allocator, Writer};
use rkyv::with::{ArchiveWith, DeserializeWith, SerializeWith};
use rkyv::{Archive, Archived, Place, Resolver};

pub type RkyvedBlockInfo = ([u8; 32], u64, [u8; 32], u64);

pub struct BlockInfoRkyv;

impl BlockInfoRkyv {
    pub fn rkyv(value: &BlockInfo) -> RkyvedBlockInfo {
        (
            value.hash.0,
            value.number,
            value.parent_hash.0,
            value.timestamp,
        )
    }

    pub fn raw(rkyved: RkyvedBlockInfo) -> BlockInfo {
        BlockInfo {
            hash: rkyved.0.into(),
            number: rkyved.1,
            parent_hash: rkyved.2.into(),
            timestamp: rkyved.3,
        }
    }
}

impl ArchiveWith<BlockInfo> for BlockInfoRkyv {
    type Archived = Archived<RkyvedBlockInfo>;
    type Resolver = Resolver<RkyvedBlockInfo>;

    fn resolve_with(field: &BlockInfo, resolver: Self::Resolver, out: Place<Self::Archived>) {
        let rkyved = BlockInfoRkyv::rkyv(field);
        <RkyvedBlockInfo as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<BlockInfo, S> for BlockInfoRkyv
where
    S: Fallible + Allocator + Writer + ?Sized,
    <S as Fallible>::Error: Source,
{
    fn serialize_with(field: &BlockInfo, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        let rkyved = BlockInfoRkyv::rkyv(field);
        <RkyvedBlockInfo as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedBlockInfo>, BlockInfo, D> for BlockInfoRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedBlockInfo>,
        deserializer: &mut D,
    ) -> Result<BlockInfo, D::Error> {
        let rkyved: RkyvedBlockInfo = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(BlockInfoRkyv::raw(rkyved))
    }
}

pub type RkyvedSystemConfig = (
    [u8; 20],
    [u8; 32],
    [u8; 32],
    u64,
    Option<u64>,
    Option<u64>,
    Option<u32>,
    Option<u32>,
    Option<u32>,
    Option<u64>,
);

pub struct SystemConfigRkyv;

impl SystemConfigRkyv {
    pub fn rkyv(value: &SystemConfig) -> RkyvedSystemConfig {
        (
            *value.batcher_address.0,
            value.overhead.to_be_bytes(),
            value.scalar.to_be_bytes(),
            value.gas_limit,
            value.base_fee_scalar,
            value.blob_base_fee_scalar,
            value.eip1559_denominator,
            value.eip1559_elasticity,
            value.operator_fee_scalar,
            value.operator_fee_constant,
        )
    }

    pub fn raw(rkyved: RkyvedSystemConfig) -> SystemConfig {
        SystemConfig {
            batcher_address: rkyved.0.into(),
            overhead: U256::from_be_bytes(rkyved.1),
            scalar: U256::from_be_bytes(rkyved.2),
            gas_limit: rkyved.3,
            base_fee_scalar: rkyved.4,
            blob_base_fee_scalar: rkyved.5,
            eip1559_denominator: rkyved.6,
            eip1559_elasticity: rkyved.7,
            operator_fee_scalar: rkyved.8,
            operator_fee_constant: rkyved.9,
        }
    }
}

impl ArchiveWith<SystemConfig> for SystemConfigRkyv {
    type Archived = Archived<RkyvedSystemConfig>;
    type Resolver = Resolver<RkyvedSystemConfig>;

    fn resolve_with(field: &SystemConfig, resolver: Self::Resolver, out: Place<Self::Archived>) {
        let rkyved = SystemConfigRkyv::rkyv(field);
        <RkyvedSystemConfig as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<SystemConfig, S> for SystemConfigRkyv
where
    S: Fallible + Allocator + Writer + ?Sized,
    <S as Fallible>::Error: Source,
{
    fn serialize_with(
        field: &SystemConfig,
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        let rkyved = SystemConfigRkyv::rkyv(field);
        <RkyvedSystemConfig as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedSystemConfig>, SystemConfig, D> for SystemConfigRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedSystemConfig>,
        deserializer: &mut D,
    ) -> Result<SystemConfig, D::Error> {
        let rkyved: RkyvedSystemConfig = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(SystemConfigRkyv::raw(rkyved))
    }
}

pub type RkyvedFrame = (ChannelId, u16, Vec<u8>, bool);

pub struct FrameRkyv;

impl FrameRkyv {
    pub fn rkyv(value: &Frame) -> RkyvedFrame {
        (value.id, value.number, value.data.clone(), value.is_last)
    }

    pub fn raw(rkyved: RkyvedFrame) -> Frame {
        Frame {
            id: rkyved.0,
            number: rkyved.1,
            data: rkyved.2,
            is_last: rkyved.3,
        }
    }
}

impl ArchiveWith<Frame> for FrameRkyv {
    type Archived = Archived<RkyvedFrame>;
    type Resolver = Resolver<RkyvedFrame>;

    fn resolve_with(field: &Frame, resolver: Self::Resolver, out: Place<Self::Archived>) {
        let rkyved = FrameRkyv::rkyv(field);
        <RkyvedFrame as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<Frame, S> for FrameRkyv
where
    S: Fallible + Allocator + Writer + ?Sized,
    <S as Fallible>::Error: Source,
{
    fn serialize_with(field: &Frame, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        let rkyved = FrameRkyv::rkyv(field);
        <RkyvedFrame as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedFrame>, Frame, D> for FrameRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedFrame>,
        deserializer: &mut D,
    ) -> Result<Frame, D::Error> {
        let rkyved: RkyvedFrame = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(FrameRkyv::raw(rkyved))
    }
}

pub type RkyvedChannel = (
    ChannelId,
    RkyvedBlockInfo,
    usize,
    bool,
    u16,
    u16,
    HashMap<u16, RkyvedFrame>,
    RkyvedBlockInfo,
);

pub struct ChannelRkyv;

impl ChannelRkyv {
    pub fn rkyv(value: &Channel) -> RkyvedChannel {
        (
            value.id,
            BlockInfoRkyv::rkyv(&value.open_block),
            value.estimated_size,
            value.closed,
            value.highest_frame_number,
            value.last_frame_number,
            value
                .inputs
                .iter()
                .map(|(k, v)| (*k, FrameRkyv::rkyv(v)))
                .collect(),
            BlockInfoRkyv::rkyv(&value.highest_l1_inclusion_block),
        )
    }

    pub fn raw(rkyved: RkyvedChannel) -> Channel {
        Channel {
            id: rkyved.0,
            open_block: BlockInfoRkyv::raw(rkyved.1),
            estimated_size: rkyved.2,
            closed: rkyved.3,
            highest_frame_number: rkyved.4,
            last_frame_number: rkyved.5,
            inputs: rkyved
                .6
                .into_iter()
                .map(|(k, v)| (k, FrameRkyv::raw(v)))
                .collect(),
            highest_l1_inclusion_block: BlockInfoRkyv::raw(rkyved.7),
        }
    }
}

impl ArchiveWith<Channel> for ChannelRkyv {
    type Archived = Archived<RkyvedChannel>;
    type Resolver = Resolver<RkyvedChannel>;

    fn resolve_with(field: &Channel, resolver: Self::Resolver, out: Place<Self::Archived>) {
        let rkyved = ChannelRkyv::rkyv(field);
        <RkyvedChannel as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<Channel, S> for ChannelRkyv
where
    S: Fallible + Allocator + Writer + ?Sized,
    <S as Fallible>::Error: Source,
{
    fn serialize_with(field: &Channel, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        let rkyved = ChannelRkyv::rkyv(field);
        <RkyvedChannel as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedChannel>, Channel, D> for ChannelRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedChannel>,
        deserializer: &mut D,
    ) -> Result<Channel, D::Error> {
        let rkyved: RkyvedChannel = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(ChannelRkyv::raw(rkyved))
    }
}

pub type RkyvedBatchReader = (Option<Vec<u8>>, Vec<u8>, usize, usize);

pub struct BatchReaderRkyv;

impl BatchReaderRkyv {
    pub fn rkyv(value: &BatchReader) -> RkyvedBatchReader {
        (
            value.data.clone(),
            value.decompressed.clone(),
            value.cursor,
            value.max_rlp_bytes_per_channel,
        )
    }

    pub fn raw(rkyved: RkyvedBatchReader) -> BatchReader {
        BatchReader {
            data: rkyved.0,
            decompressed: rkyved.1,
            cursor: rkyved.2,
            max_rlp_bytes_per_channel: rkyved.3,
        }
    }
}

impl ArchiveWith<BatchReader> for BatchReaderRkyv {
    type Archived = Archived<RkyvedBatchReader>;
    type Resolver = Resolver<RkyvedBatchReader>;

    fn resolve_with(field: &BatchReader, resolver: Self::Resolver, out: Place<Self::Archived>) {
        let rkyved = BatchReaderRkyv::rkyv(field);
        <RkyvedBatchReader as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<BatchReader, S> for BatchReaderRkyv
where
    S: Fallible + Allocator + Writer + ?Sized,
    <S as Fallible>::Error: Source,
{
    fn serialize_with(field: &BatchReader, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        let rkyved = BatchReaderRkyv::rkyv(field);
        <RkyvedBatchReader as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedBatchReader>, BatchReader, D> for BatchReaderRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedBatchReader>,
        deserializer: &mut D,
    ) -> Result<BatchReader, D::Error> {
        let rkyved: RkyvedBatchReader = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(BatchReaderRkyv::raw(rkyved))
    }
}

pub type RkyvedSingleBatch = ([u8; 32], u64, [u8; 32], u64, Vec<Vec<u8>>);

pub struct SingleBatchRkyv;

impl SingleBatchRkyv {
    pub fn rkyv(value: &SingleBatch) -> RkyvedSingleBatch {
        (
            value.parent_hash.0,
            value.epoch_num,
            value.epoch_hash.0,
            value.timestamp,
            value.transactions.iter().map(|v| v.to_vec()).collect(),
        )
    }

    pub fn raw(rkyved: RkyvedSingleBatch) -> SingleBatch {
        SingleBatch {
            parent_hash: rkyved.0.into(),
            epoch_num: rkyved.1,
            epoch_hash: rkyved.2.into(),
            timestamp: rkyved.3,
            transactions: rkyved.4.into_iter().map(|v| v.into()).collect(),
        }
    }
}

impl ArchiveWith<SingleBatch> for SingleBatchRkyv {
    type Archived = Archived<RkyvedSingleBatch>;
    type Resolver = Resolver<RkyvedSingleBatch>;

    fn resolve_with(field: &SingleBatch, resolver: Self::Resolver, out: Place<Self::Archived>) {
        let rkyved = SingleBatchRkyv::rkyv(field);
        <RkyvedSingleBatch as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<SingleBatch, S> for SingleBatchRkyv
where
    S: Fallible + Allocator + Writer + ?Sized,
    <S as Fallible>::Error: Source,
{
    fn serialize_with(field: &SingleBatch, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        let rkyved = SingleBatchRkyv::rkyv(field);
        <RkyvedSingleBatch as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedSingleBatch>, SingleBatch, D> for SingleBatchRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedSingleBatch>,
        deserializer: &mut D,
    ) -> Result<SingleBatch, D::Error> {
        let rkyved: RkyvedSingleBatch = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(SingleBatchRkyv::raw(rkyved))
    }
}

pub type RkyvedSpanBatchElement = (u64, u64, Vec<Vec<u8>>);

pub type RkyvedSpanBatchTransactions = (
    u64,
    Vec<u8>,
    Vec<[u8; 65]>,
    Vec<u64>,
    Vec<u64>,
    Vec<[u8; 20]>,
    Vec<Vec<u8>>,
    Vec<u8>,
    Vec<u8>,
    u64,
);

pub type RkyvedSpanBatch = (
    [u8; 20],
    [u8; 20],
    u64,
    u64,
    Vec<RkyvedSpanBatchElement>,
    Vec<u8>,
    Vec<u64>,
    RkyvedSpanBatchTransactions,
);

pub struct SpanBatchRkyv;

impl SpanBatchRkyv {
    pub fn rkyv(value: &SpanBatch) -> RkyvedSpanBatch {
        (
            value.parent_check.0,
            value.l1_origin_check.0,
            value.genesis_timestamp,
            value.chain_id,
            value
                .batches
                .iter()
                .map(|v| {
                    (
                        v.epoch_num,
                        v.timestamp,
                        v.transactions.iter().map(|v| v.to_vec()).collect(),
                    )
                })
                .collect(),
            value.origin_bits.0.clone(),
            value.block_tx_counts.clone(),
            (
                value.txs.total_block_tx_count,
                value.txs.contract_creation_bits.0.clone(),
                value.txs.tx_sigs.iter().map(|s| s.as_bytes()).collect(),
                value.txs.tx_nonces.clone(),
                value.txs.tx_gases.clone(),
                value.txs.tx_tos.iter().map(|v| *v.0).collect(),
                value.txs.tx_datas.clone(),
                value.txs.protected_bits.0.clone(),
                value.txs.tx_types.iter().map(|v| v.ty()).collect(),
                value.txs.legacy_tx_count,
            ),
        )
    }

    pub fn raw(rkyved: RkyvedSpanBatch) -> SpanBatch {
        SpanBatch {
            parent_check: rkyved.0.into(),
            l1_origin_check: rkyved.1.into(),
            genesis_timestamp: rkyved.2,
            chain_id: rkyved.3,
            batches: rkyved
                .4
                .into_iter()
                .map(|v| SpanBatchElement {
                    epoch_num: v.0,
                    timestamp: v.1,
                    transactions: v.2.into_iter().map(|b| b.into()).collect(),
                })
                .collect(),
            origin_bits: SpanBatchBits(rkyved.5),
            block_tx_counts: rkyved.6,
            txs: SpanBatchTransactions {
                total_block_tx_count: rkyved.7 .0,
                contract_creation_bits: SpanBatchBits(rkyved.7 .1),
                tx_sigs: rkyved
                    .7
                     .2
                    .into_iter()
                    .map(|s| Signature::from_raw_array(&s).unwrap())
                    .collect(),
                tx_nonces: rkyved.7 .3,
                tx_gases: rkyved.7 .4,
                tx_tos: rkyved.7 .5.into_iter().map(|a| a.into()).collect(),
                tx_datas: rkyved.7 .6,
                protected_bits: SpanBatchBits(rkyved.7 .7),
                tx_types: rkyved
                    .7
                     .8
                    .into_iter()
                    .map(|t| t.try_into().unwrap())
                    .collect(),
                legacy_tx_count: rkyved.7 .9,
            },
        }
    }
}

impl ArchiveWith<SpanBatch> for SpanBatchRkyv {
    type Archived = Archived<RkyvedSpanBatch>;
    type Resolver = Resolver<RkyvedSpanBatch>;

    fn resolve_with(field: &SpanBatch, resolver: Self::Resolver, out: Place<Self::Archived>) {
        let rkyved = SpanBatchRkyv::rkyv(field);
        <RkyvedSpanBatch as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<SpanBatch, S> for SpanBatchRkyv
where
    S: Fallible + Allocator + Writer + ?Sized,
    <S as Fallible>::Error: Source,
{
    fn serialize_with(field: &SpanBatch, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        let rkyved = SpanBatchRkyv::rkyv(field);
        <RkyvedSpanBatch as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedSpanBatch>, SpanBatch, D> for SpanBatchRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedSpanBatch>,
        deserializer: &mut D,
    ) -> Result<SpanBatch, D::Error> {
        let rkyved: RkyvedSpanBatch = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(SpanBatchRkyv::raw(rkyved))
    }
}

pub type RkyvedBatchWithInclusionBlock = (
    RkyvedBlockInfo,
    Option<RkyvedSingleBatch>,
    Option<RkyvedSpanBatch>,
);

pub struct BatchWithInclusionBlockRkyv;

impl BatchWithInclusionBlockRkyv {
    pub fn rkyv(value: &BatchWithInclusionBlock) -> RkyvedBatchWithInclusionBlock {
        let (single, span) = match &value.batch {
            Batch::Single(single) => (Some(SingleBatchRkyv::rkyv(single)), None),
            Batch::Span(span) => (None, Some(SpanBatchRkyv::rkyv(span))),
        };
        (BlockInfoRkyv::rkyv(&value.inclusion_block), single, span)
    }

    pub fn raw(rkyved: RkyvedBatchWithInclusionBlock) -> BatchWithInclusionBlock {
        BatchWithInclusionBlock {
            inclusion_block: BlockInfoRkyv::raw(rkyved.0),
            batch: match (rkyved.1, rkyved.2) {
                (Some(single), None) => Batch::Single(SingleBatchRkyv::raw(single)),
                (None, Some(span)) => Batch::Span(SpanBatchRkyv::raw(span)),
                _ => unreachable!("Bad Batch rkyv."),
            },
        }
    }
}

impl ArchiveWith<BatchWithInclusionBlock> for BatchWithInclusionBlockRkyv {
    type Archived = Archived<RkyvedBatchWithInclusionBlock>;
    type Resolver = Resolver<RkyvedBatchWithInclusionBlock>;

    fn resolve_with(
        field: &BatchWithInclusionBlock,
        resolver: Self::Resolver,
        out: Place<Self::Archived>,
    ) {
        let rkyved = BatchWithInclusionBlockRkyv::rkyv(field);
        <RkyvedBatchWithInclusionBlock as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<BatchWithInclusionBlock, S> for BatchWithInclusionBlockRkyv
where
    S: Fallible + Allocator + Writer + ?Sized,
    <S as Fallible>::Error: Source,
{
    fn serialize_with(
        field: &BatchWithInclusionBlock,
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        let rkyved = BatchWithInclusionBlockRkyv::rkyv(field);
        <RkyvedBatchWithInclusionBlock as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedBatchWithInclusionBlock>, BatchWithInclusionBlock, D>
    for BatchWithInclusionBlockRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedBatchWithInclusionBlock>,
        deserializer: &mut D,
    ) -> Result<BatchWithInclusionBlock, D::Error> {
        let rkyved: RkyvedBatchWithInclusionBlock =
            rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(BatchWithInclusionBlockRkyv::raw(rkyved))
    }
}

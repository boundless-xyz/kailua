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

use alloy_consensus::Header;
use alloy_eips::eip4895::Withdrawal;
use alloy_eips::eip7685::Requests;
use alloy_eips::{BlockNumHash, Typed2718};
use alloy_evm::block::BlockExecutionResult;
use alloy_primitives::{Bytes, Sealable, Signature, U256};
use alloy_rpc_types_engine::PayloadAttributes;
use kona_driver::{PipelineCursor, TipCursor};
use kona_executor::BlockBuildingOutcome;
use kona_genesis::SystemConfig;
use kona_protocol::{
    Batch, BatchReader, BatchWithInclusionBlock, BlockInfo, Channel, ChannelId, Frame, L2BlockInfo,
    OpAttributesWithParent, SingleBatch, SpanBatch, SpanBatchBits, SpanBatchElement,
    SpanBatchTransactions,
};
use op_alloy_consensus::OpReceiptEnvelope;
use op_alloy_rpc_types_engine::OpPayloadAttributes;
use rkyv::rancor::{Fallible, Source};
use rkyv::ser::{Allocator, Writer};
use rkyv::with::{ArchiveWith, DeserializeWith, SerializeWith};
use rkyv::{Archive, Archived, Place, Resolver};

pub fn sorted<T: Ord>(mut values: Vec<T>) -> Vec<T> {
    values.sort();
    values
}

pub fn sorted_by_key<T1: Ord + Copy, T2>(mut values: Vec<(T1, T2)>) -> Vec<(T1, T2)> {
    values.sort_by_key(|(k, _)| *k);
    values
}

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
    Vec<(u16, RkyvedFrame)>,
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
            sorted(
                value
                    .inputs
                    .iter()
                    .map(|(k, v)| (*k, FrameRkyv::rkyv(v)))
                    .collect(),
            ),
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

pub type RkyvedL2BlockInfo = (RkyvedBlockInfo, u64, [u8; 32], u64);

pub struct L2BlockInfoRkyv;

impl L2BlockInfoRkyv {
    pub fn rkyv(value: &L2BlockInfo) -> RkyvedL2BlockInfo {
        (
            BlockInfoRkyv::rkyv(&value.block_info),
            value.l1_origin.number,
            value.l1_origin.hash.0,
            value.seq_num,
        )
    }

    pub fn raw(rkyved: RkyvedL2BlockInfo) -> L2BlockInfo {
        L2BlockInfo {
            block_info: BlockInfoRkyv::raw(rkyved.0),
            l1_origin: BlockNumHash {
                number: rkyved.1,
                hash: rkyved.2.into(),
            },
            seq_num: rkyved.3,
        }
    }
}

pub type RkyvedWithdrawal = (u64, u64, [u8; 20], u64);

pub struct WithdrawalRkyv;

impl WithdrawalRkyv {
    pub fn rkyv(value: &Withdrawal) -> RkyvedWithdrawal {
        (
            value.index,
            value.validator_index,
            *value.address.0,
            value.amount,
        )
    }

    pub fn raw(rkyved: RkyvedWithdrawal) -> Withdrawal {
        Withdrawal {
            index: rkyved.0,
            validator_index: rkyved.1,
            address: rkyved.2.into(),
            amount: rkyved.3,
        }
    }
}

pub type RkyvedPayloadAttributes = (
    u64,
    [u8; 32],
    [u8; 20],
    Option<Vec<RkyvedWithdrawal>>,
    Option<[u8; 32]>,
);

pub struct PayloadAttributesRkyv;

impl PayloadAttributesRkyv {
    pub fn rkyv(value: &PayloadAttributes) -> RkyvedPayloadAttributes {
        (
            value.timestamp,
            value.prev_randao.0,
            *value.suggested_fee_recipient.0,
            value
                .withdrawals
                .as_ref()
                .map(|v| v.iter().map(WithdrawalRkyv::rkyv).collect()),
            value.parent_beacon_block_root.as_ref().map(|r| r.0),
        )
    }

    pub fn raw(rkyved: RkyvedPayloadAttributes) -> PayloadAttributes {
        PayloadAttributes {
            timestamp: rkyved.0,
            prev_randao: rkyved.1.into(),
            suggested_fee_recipient: rkyved.2.into(),
            withdrawals: rkyved
                .3
                .map(|v| v.into_iter().map(WithdrawalRkyv::raw).collect()),
            parent_beacon_block_root: rkyved.4.map(|v| v.into()),
        }
    }
}

pub type RkyvedOpPayloadAttributes = (
    RkyvedPayloadAttributes,
    Option<Vec<Vec<u8>>>,
    Option<bool>,
    Option<u64>,
    Option<[u8; 8]>,
);

pub struct OpPayloadAttributesRkyv;

impl OpPayloadAttributesRkyv {
    pub fn rkyv(value: &OpPayloadAttributes) -> RkyvedOpPayloadAttributes {
        (
            PayloadAttributesRkyv::rkyv(&value.payload_attributes),
            value
                .transactions
                .as_ref()
                .map(|v| v.iter().map(|v| v.to_vec()).collect()),
            value.no_tx_pool,
            value.gas_limit,
            value.eip_1559_params.map(|v| v.0),
        )
    }

    pub fn raw(rkyved: RkyvedOpPayloadAttributes) -> OpPayloadAttributes {
        OpPayloadAttributes {
            payload_attributes: PayloadAttributesRkyv::raw(rkyved.0),
            transactions: rkyved.1.map(|v| v.into_iter().map(|v| v.into()).collect()),
            no_tx_pool: rkyved.2,
            gas_limit: rkyved.3,
            eip_1559_params: rkyved.4.map(|v| v.into()),
        }
    }
}

pub type RkyvedOpAttributesWithParent = (
    RkyvedOpPayloadAttributes,
    RkyvedL2BlockInfo,
    RkyvedBlockInfo,
    bool,
);

pub struct OpAttributesWithParentRkyv;

impl OpAttributesWithParentRkyv {
    pub fn rkyv(value: &OpAttributesWithParent) -> RkyvedOpAttributesWithParent {
        (
            OpPayloadAttributesRkyv::rkyv(&value.inner),
            L2BlockInfoRkyv::rkyv(&value.parent),
            BlockInfoRkyv::rkyv(&value.l1_origin),
            value.is_last_in_span,
        )
    }

    pub fn raw(rkyved: RkyvedOpAttributesWithParent) -> OpAttributesWithParent {
        OpAttributesWithParent {
            inner: OpPayloadAttributesRkyv::raw(rkyved.0),
            parent: L2BlockInfoRkyv::raw(rkyved.1),
            l1_origin: BlockInfoRkyv::raw(rkyved.2),
            is_last_in_span: rkyved.3,
        }
    }
}

impl ArchiveWith<OpAttributesWithParent> for OpAttributesWithParentRkyv {
    type Archived = Archived<RkyvedOpAttributesWithParent>;
    type Resolver = Resolver<RkyvedOpAttributesWithParent>;

    fn resolve_with(
        field: &OpAttributesWithParent,
        resolver: Self::Resolver,
        out: Place<Self::Archived>,
    ) {
        let rkyved = OpAttributesWithParentRkyv::rkyv(field);
        <RkyvedOpAttributesWithParent as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<OpAttributesWithParent, S> for OpAttributesWithParentRkyv
where
    S: Fallible + Allocator + Writer + ?Sized,
    <S as Fallible>::Error: Source,
{
    fn serialize_with(
        field: &OpAttributesWithParent,
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        let rkyved = OpAttributesWithParentRkyv::rkyv(field);
        <RkyvedOpAttributesWithParent as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedOpAttributesWithParent>, OpAttributesWithParent, D>
    for OpAttributesWithParentRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedOpAttributesWithParent>,
        deserializer: &mut D,
    ) -> Result<OpAttributesWithParent, D::Error> {
        let rkyved: RkyvedOpAttributesWithParent =
            rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(OpAttributesWithParentRkyv::raw(rkyved))
    }
}

pub type RkyvedHeaderHashes = (
    [u8; 32],
    [u8; 32],
    [u8; 20],
    [u8; 32],
    [u8; 32],
    [u8; 32],
    [u8; 256],
    [u8; 32],
    Option<[u8; 32]>,
);

pub type RkyvedHeader = (
    RkyvedHeaderHashes,
    [u8; 32],
    u64,
    u64,
    u64,
    u64,
    Vec<u8>,
    [u8; 8],
    Option<u64>,
    Option<u64>,
    Option<u64>,
    Option<[u8; 32]>,
    Option<[u8; 32]>,
);

pub struct HeaderRkyv;

impl HeaderRkyv {
    pub fn rkyv(value: &Header) -> RkyvedHeader {
        (
            (
                value.parent_hash.0,
                value.ommers_hash.0,
                *value.beneficiary.0,
                value.state_root.0,
                value.transactions_root.0,
                value.receipts_root.0,
                *value.logs_bloom.0,
                value.mix_hash.0,
                value.withdrawals_root.map(|v| v.0),
            ),
            value.difficulty.to_be_bytes(),
            value.number,
            value.gas_limit,
            value.gas_used,
            value.timestamp,
            value.extra_data.to_vec(),
            value.nonce.0,
            value.base_fee_per_gas,
            value.blob_gas_used,
            value.excess_blob_gas,
            value.parent_beacon_block_root.map(|v| v.0),
            value.requests_hash.map(|v| v.0),
        )
    }

    pub fn raw(rkyved: RkyvedHeader) -> Header {
        Header {
            parent_hash: rkyved.0 .0.into(),
            ommers_hash: rkyved.0 .1.into(),
            beneficiary: rkyved.0 .2.into(),
            state_root: rkyved.0 .3.into(),
            transactions_root: rkyved.0 .4.into(),
            receipts_root: rkyved.0 .5.into(),
            logs_bloom: rkyved.0 .6.into(),
            mix_hash: rkyved.0 .7.into(),
            withdrawals_root: rkyved.0 .8.map(|v| v.into()),
            difficulty: U256::from_be_bytes(rkyved.1),
            number: rkyved.2,
            gas_limit: rkyved.3,
            gas_used: rkyved.4,
            timestamp: rkyved.5,
            extra_data: rkyved.6.into(),
            nonce: rkyved.7.into(),
            base_fee_per_gas: rkyved.8,
            blob_gas_used: rkyved.9,
            excess_blob_gas: rkyved.10,
            parent_beacon_block_root: rkyved.11.map(|v| v.into()),
            requests_hash: rkyved.12.map(|v| v.into()),
        }
    }
}

pub type OPBlockExecutionResult = BlockExecutionResult<OpReceiptEnvelope>;
pub type RkyvedOPBlockExecutionResult = (Vec<Vec<u8>>, Vec<Vec<u8>>, u64);

pub struct OPBlockExecutionResultRkyv;

impl OPBlockExecutionResultRkyv {
    pub fn rkyv(value: &OPBlockExecutionResult) -> RkyvedOPBlockExecutionResult {
        (
            value.receipts.iter().map(alloy_rlp::encode).collect(),
            value
                .requests
                .clone()
                .take()
                .into_iter()
                .map(|v| v.to_vec())
                .collect(),
            value.gas_used,
        )
    }

    pub fn raw(rkyved: RkyvedOPBlockExecutionResult) -> OPBlockExecutionResult {
        OPBlockExecutionResult {
            receipts: rkyved
                .0
                .into_iter()
                .map(|v| alloy_rlp::decode_exact(&v).unwrap())
                .collect(),
            requests: Requests::new(rkyved.1.into_iter().map(|v| v.into()).collect()),
            gas_used: rkyved.2,
        }
    }
}

pub type RkyvedBlockBuildingOutcome = (RkyvedHeader, RkyvedOPBlockExecutionResult);

pub struct BlockBuildingOutcomeRkyv;

impl BlockBuildingOutcomeRkyv {
    pub fn rkyv(value: &BlockBuildingOutcome) -> RkyvedBlockBuildingOutcome {
        (
            HeaderRkyv::rkyv(value.header.as_ref()),
            OPBlockExecutionResultRkyv::rkyv(&value.execution_result),
        )
    }

    pub fn raw(rkyved: RkyvedBlockBuildingOutcome) -> BlockBuildingOutcome {
        BlockBuildingOutcome {
            header: HeaderRkyv::raw(rkyved.0).seal_slow(),
            execution_result: OPBlockExecutionResultRkyv::raw(rkyved.1),
        }
    }
}

pub type HeadArtifacts = (BlockBuildingOutcome, Vec<Bytes>);
pub type RkyvedHeadArtifacts = (RkyvedBlockBuildingOutcome, Vec<Vec<u8>>);

pub struct HeadArtifactsRkyv;

impl HeadArtifactsRkyv {
    pub fn rkyv(value: &HeadArtifacts) -> RkyvedHeadArtifacts {
        (
            BlockBuildingOutcomeRkyv::rkyv(&value.0),
            value.1.iter().map(|v| v.to_vec()).collect(),
        )
    }

    pub fn raw(rkyved: RkyvedHeadArtifacts) -> HeadArtifacts {
        (
            BlockBuildingOutcomeRkyv::raw(rkyved.0),
            rkyved.1.into_iter().map(|v| v.into()).collect(),
        )
    }
}

impl ArchiveWith<HeadArtifacts> for HeadArtifactsRkyv {
    type Archived = Archived<RkyvedHeadArtifacts>;
    type Resolver = Resolver<RkyvedHeadArtifacts>;

    fn resolve_with(field: &HeadArtifacts, resolver: Self::Resolver, out: Place<Self::Archived>) {
        let rkyved = HeadArtifactsRkyv::rkyv(field);
        <RkyvedHeadArtifacts as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<HeadArtifacts, S> for HeadArtifactsRkyv
where
    S: Fallible + Allocator + Writer + ?Sized,
    <S as Fallible>::Error: Source,
{
    fn serialize_with(
        field: &HeadArtifacts,
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        let rkyved = HeadArtifactsRkyv::rkyv(field);
        <RkyvedHeadArtifacts as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedHeadArtifacts>, HeadArtifacts, D> for HeadArtifactsRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedHeadArtifacts>,
        deserializer: &mut D,
    ) -> Result<HeadArtifacts, D::Error> {
        let rkyved: RkyvedHeadArtifacts = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(HeadArtifactsRkyv::raw(rkyved))
    }
}

pub type RkyvedTipCursor = (RkyvedL2BlockInfo, RkyvedHeader, [u8; 32]);

pub struct TipCursorRkyv;

impl TipCursorRkyv {
    pub fn rkyv(value: &TipCursor) -> RkyvedTipCursor {
        (
            L2BlockInfoRkyv::rkyv(&value.l2_safe_head),
            HeaderRkyv::rkyv(value.l2_safe_head_header.as_ref()),
            value.l2_safe_head_output_root.0,
        )
    }

    pub fn raw(rkyved: RkyvedTipCursor) -> TipCursor {
        TipCursor {
            l2_safe_head: L2BlockInfoRkyv::raw(rkyved.0),
            l2_safe_head_header: HeaderRkyv::raw(rkyved.1).seal_slow(),
            l2_safe_head_output_root: rkyved.2.into(),
        }
    }
}

pub type RkyvedPipelineCursor = (
    usize,
    u64,
    RkyvedBlockInfo,
    Vec<u64>,
    Vec<(u64, RkyvedBlockInfo)>,
    Vec<(u64, RkyvedTipCursor)>,
);

pub struct PipelineCursorRkyv;

impl PipelineCursorRkyv {
    pub fn rkyv(value: &PipelineCursor) -> RkyvedPipelineCursor {
        (
            value.capacity,
            value.channel_timeout,
            BlockInfoRkyv::rkyv(&value.origin),
            value.origins.clone().into(),
            sorted(
                value
                    .origin_infos
                    .iter()
                    .map(|(k, v)| (*k, BlockInfoRkyv::rkyv(v)))
                    .collect(),
            ),
            sorted_by_key(
                value
                    .tips
                    .iter()
                    .map(|(k, v)| (*k, TipCursorRkyv::rkyv(v)))
                    .collect(),
            ),
        )
    }

    pub fn raw(rkyved: RkyvedPipelineCursor) -> PipelineCursor {
        PipelineCursor {
            capacity: rkyved.0,
            channel_timeout: rkyved.1,
            origin: BlockInfoRkyv::raw(rkyved.2),
            origins: rkyved.3.into(),
            origin_infos: rkyved
                .4
                .into_iter()
                .map(|(k, v)| (k, BlockInfoRkyv::raw(v)))
                .collect(),
            tips: rkyved
                .5
                .into_iter()
                .map(|(k, v)| (k, TipCursorRkyv::raw(v)))
                .collect(),
        }
    }
}

impl ArchiveWith<PipelineCursor> for PipelineCursorRkyv {
    type Archived = Archived<RkyvedPipelineCursor>;
    type Resolver = Resolver<RkyvedPipelineCursor>;

    fn resolve_with(field: &PipelineCursor, resolver: Self::Resolver, out: Place<Self::Archived>) {
        let rkyved = PipelineCursorRkyv::rkyv(field);
        <RkyvedPipelineCursor as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<PipelineCursor, S> for PipelineCursorRkyv
where
    S: Fallible + Allocator + Writer + ?Sized,
    <S as Fallible>::Error: Source,
{
    fn serialize_with(
        field: &PipelineCursor,
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        let rkyved = PipelineCursorRkyv::rkyv(field);
        <RkyvedPipelineCursor as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedPipelineCursor>, PipelineCursor, D> for PipelineCursorRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedPipelineCursor>,
        deserializer: &mut D,
    ) -> Result<PipelineCursor, D::Error> {
        let rkyved: RkyvedPipelineCursor = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(PipelineCursorRkyv::raw(rkyved))
    }
}

pub type IdChannel = (ChannelId, Channel);
pub type RkyvedIdChannel = (ChannelId, RkyvedChannel);

pub struct IdChannelRkyv;

impl IdChannelRkyv {
    pub fn rkyv(value: &IdChannel) -> RkyvedIdChannel {
        (value.0, ChannelRkyv::rkyv(&value.1))
    }

    pub fn raw(rkyved: RkyvedIdChannel) -> IdChannel {
        (rkyved.0, ChannelRkyv::raw(rkyved.1))
    }
}

impl ArchiveWith<IdChannel> for IdChannelRkyv {
    type Archived = Archived<RkyvedIdChannel>;
    type Resolver = Resolver<RkyvedIdChannel>;

    fn resolve_with(field: &IdChannel, resolver: Self::Resolver, out: Place<Self::Archived>) {
        let rkyved = IdChannelRkyv::rkyv(field);
        <RkyvedIdChannel as Archive>::resolve(&rkyved, resolver, out);
    }
}

impl<S> SerializeWith<IdChannel, S> for IdChannelRkyv
where
    S: Fallible + Allocator + Writer + ?Sized,
    <S as Fallible>::Error: Source,
{
    fn serialize_with(field: &IdChannel, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        let rkyved = IdChannelRkyv::rkyv(field);
        <RkyvedIdChannel as rkyv::Serialize<S>>::serialize(&rkyved, serializer)
    }
}

impl<D> DeserializeWith<Archived<RkyvedIdChannel>, IdChannel, D> for IdChannelRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: Source,
{
    fn deserialize_with(
        field: &Archived<RkyvedIdChannel>,
        deserializer: &mut D,
    ) -> Result<IdChannel, D::Error> {
        let rkyved: RkyvedIdChannel = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(IdChannelRkyv::raw(rkyved))
    }
}

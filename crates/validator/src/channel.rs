// Copyright 2024 Boundless Foundation, Inc.
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

use alloy::primitives::FixedBytes;
use kailua_kona::precondition::proposal::ProposalPrecondition;
use risc0_zkvm::Receipt;
use tokio::sync::mpsc::{channel, Receiver, Sender};

/// A channel for two-way communication
#[derive(Debug)]
pub struct DuplexChannel<T> {
    /// Messages coming in
    pub receiver: Receiver<T>,
    /// Messages going out
    pub sender: Sender<T>,
}

impl<T> DuplexChannel<T> {
    /// Returns a pair of duplex channel instances, one for each endpoint
    pub fn new_pair(buffer: usize) -> (Self, Self) {
        let pair_0 = channel(buffer);
        let pair_1 = channel(buffer);
        let channel_0 = Self {
            receiver: pair_1.1,
            sender: pair_0.0,
        };
        let channel_1 = Self {
            receiver: pair_0.1,
            sender: pair_1.0,
        };
        (channel_0, channel_1)
    }
}

/// Work items exchanged between the proposal follower and the proof generator.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Message {
    /// A request to prove one output transition claimed by a proposal.
    Proposal {
        /// Local index of the proposal to prove.
        index: u64,
        /// Precondition binding a validity proof to the proposal's published blob data.
        precondition_validation_data: Option<ProposalPrecondition>,
        /// L1 block hash bounding the data available to derivation.
        l1_head: FixedBytes<32>,
        /// Hash of the L2 block the proof starts from.
        agreed_l2_head_hash: FixedBytes<32>,
        /// Output root of the agreed L2 block.
        agreed_l2_output_root: FixedBytes<32>,
        /// L2 block number of the output claim to decide.
        claimed_l2_block_number: u64,
        /// The output root claim to prove or disprove.
        claimed_l2_output_root: FixedBytes<32>,
    },
    /// A completed proof for the proposal at the given local index, or `None` if the
    /// requested L1 head had insufficient data to derive the claimed block.
    Proof(u64, Option<Receipt>),
}

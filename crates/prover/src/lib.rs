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

use alloy_primitives::B256;
use async_channel::Sender;
use kailua_kona::driver::CachedDriver;
use kailua_kona::executor::Execution;

pub mod args;
pub mod channel;
pub mod client;
pub mod config;
pub mod driver;
#[cfg(feature = "celestia")]
pub mod hana;
#[cfg(feature = "eigen")]
pub mod hokulea;
pub mod kv;
pub mod preflight;
pub mod proof;
pub mod prove;
pub mod risczero;
pub mod tasks;

#[derive(Debug, thiserror::Error)]
pub enum ProvingError {
    #[error("DerivationProofError error: execution proofs {0}")]
    SkippingDerivation(usize),

    #[error("NotSeekingProof error: preloaded {0} streamed {1}")]
    NotSeekingProof(
        usize,
        usize,
        Vec<Vec<Execution>>,
        Box<Option<CachedDriver>>,
        Option<Sender<CachedDriver>>,
        B256,
    ),

    #[error("NotAwaitingProof error")]
    NotAwaitingProof,

    #[error("BlockCountError error: count {0} limit {1}")]
    BlockCountError(
        usize,
        usize,
        Vec<Vec<Execution>>,
        Box<Option<CachedDriver>>,
        Option<Sender<CachedDriver>>,
    ),

    #[error("WitnessSizeError error: preloaded {0} streamed {1} limit {2}")]
    WitnessSizeError(
        usize,
        usize,
        usize,
        Vec<Vec<Execution>>,
        Box<Option<CachedDriver>>,
        Option<Sender<CachedDriver>>,
    ),

    #[error("OtherError error: {0:?}")]
    OtherError(anyhow::Error),
}

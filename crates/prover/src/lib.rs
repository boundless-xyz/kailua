// Copyright 2024, 2025 Boundless Foundation, Inc.
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

#![recursion_limit = "256"]

use alloy_primitives::B256;
use async_channel::Sender;
use kailua_kona::driver::CachedDriver;
#[cfg(feature = "experimental")]
use kailua_kona::evm::partial::PartialExecution;
use kailua_kona::executor::Execution;
use std::time::SystemTime;

pub mod args;
pub mod channel;
pub mod client;
pub mod config;
pub mod driver;
#[cfg(feature = "celestia")]
pub mod hana;
pub mod hint_backoff;
pub mod hint_handler;
#[cfg(feature = "eigen")]
pub mod hokulea;
pub mod kv;
pub mod preflight;
pub mod profiling;
pub mod proof;
pub mod prove;
pub mod risczero;
pub mod tasks;

#[derive(Debug, thiserror::Error)]
pub enum ProvingError {
    #[error("DerivationProofError error: execution proofs {0}")]
    SkippingDerivation(usize),

    #[error("NotSeekingProof error: preloaded {preloaded_size} streamed {streamed_size}")]
    NotSeekingProof {
        preloaded_size: usize,
        streamed_size: usize,
        executions: Vec<Vec<Execution>>,
        #[cfg(feature = "experimental")]
        partials: Vec<Vec<PartialExecution>>,
        derivation_cache: Box<Option<CachedDriver>>,
        derivation_trace: Option<Sender<CachedDriver>>,
        derivation_trace_hash: B256,
    },

    #[error("NotAwaitingProof error")]
    NotAwaitingProof,

    #[error("BlockCountError error: count {count} limit {limit}")]
    BlockCountError {
        count: usize,
        limit: usize,
        executions: Vec<Vec<Execution>>,
        #[cfg(feature = "experimental")]
        partials: Vec<Vec<PartialExecution>>,
        derivation_cache: Box<Option<CachedDriver>>,
        derivation_trace: Option<Sender<CachedDriver>>,
    },

    #[error(
        "WitnessSizeError error: preloaded {preloaded_size} streamed {streamed_size} limit {limit}"
    )]
    WitnessSizeError {
        preloaded_size: usize,
        streamed_size: usize,
        limit: usize,
        executions: Vec<Vec<Execution>>,
        #[cfg(feature = "experimental")]
        partials: Vec<Vec<PartialExecution>>,
        derivation_cache: Box<Option<CachedDriver>>,
        derivation_trace: Option<Sender<CachedDriver>>,
    },

    #[error("ProvingTimeout error")]
    ProvingTimeout,

    #[error("OtherError error: {0:?}")]
    OtherError(anyhow::Error),
}

impl ProvingError {
    pub fn with_driver_cache(self, driver_cache: Option<CachedDriver>) -> Self {
        match self {
            ProvingError::NotSeekingProof {
                preloaded_size,
                streamed_size,
                executions,
                #[cfg(feature = "experimental")]
                partials,
                derivation_cache: _,
                derivation_trace,
                derivation_trace_hash,
            } => ProvingError::NotSeekingProof {
                preloaded_size,
                streamed_size,
                executions,
                #[cfg(feature = "experimental")]
                partials,
                derivation_cache: Box::new(driver_cache),
                derivation_trace,
                derivation_trace_hash,
            },
            ProvingError::BlockCountError {
                count,
                limit,
                executions,
                #[cfg(feature = "experimental")]
                partials,
                derivation_cache: _,
                derivation_trace,
            } => ProvingError::BlockCountError {
                count,
                limit,
                executions,
                #[cfg(feature = "experimental")]
                partials,
                derivation_cache: Box::new(driver_cache),
                derivation_trace,
            },
            ProvingError::WitnessSizeError {
                preloaded_size,
                streamed_size,
                limit,
                executions,
                #[cfg(feature = "experimental")]
                partials,
                derivation_cache: _,
                derivation_trace,
            } => ProvingError::WitnessSizeError {
                preloaded_size,
                streamed_size,
                limit,
                executions,
                #[cfg(feature = "experimental")]
                partials,
                derivation_cache: Box::new(driver_cache),
                derivation_trace,
            },
            err => err,
        }
    }
}

impl From<anyhow::Error> for ProvingError {
    fn from(err: anyhow::Error) -> Self {
        ProvingError::OtherError(err)
    }
}

pub fn current_time() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

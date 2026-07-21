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
#![cfg_attr(not(test), warn(missing_docs))]

//! Host-side proof generation orchestrator for Kailua.
//!
//! Proving a claim proceeds in stages: [preflight] runs the claimed state transition natively to
//! populate a disk preimage store, [client] re-runs the guest program logic to assemble and
//! validate the witness, and [risczero] computes the FPVM proof locally or through the Bonsai or
//! Boundless proving services. [prove] drives the pipeline end to end, using [tasks] to
//! decompose workloads that exceed witness-size or block-count limits into execution-only,
//! derivation-tail, and stitching sub-proofs.

use alloy_primitives::B256;
use async_channel::Sender;
use kailua_kona::driver::CachedDriver;
#[cfg(feature = "experimental")]
use kailua_kona::evm::partial::PartialExecution;
use kailua_kona::executor::Execution;
use std::time::SystemTime;

/// Command-line argument types shared by the proving CLI and its recursive self-invocations.
pub mod args;
/// Convenience alias for a paired async channel.
pub mod channel;
/// Native re-implementations of the guest proving clients, used for witness generation.
pub mod client;
/// Materialization of rollup and L1 chain configurations as files readable by kona's host.
pub mod config;
/// Disk caching and channel signaling of derivation pipeline snapshots.
pub mod driver;
/// Celestia (Hana) host-side witness support.
#[cfg(feature = "celestia")]
pub mod hana;
pub mod hint_backoff;
pub mod hint_handler;
/// EigenDA (Hokulea) host-side witness support.
#[cfg(feature = "eigen")]
pub mod hokulea;
/// Preimage key-value stores shared across concurrent proving tasks.
pub mod kv;
/// Preimage store population and execution trace collection ahead of proving.
pub mod preflight;
/// Proving workload statistics collection and CSV export.
pub mod profiling;
/// Proof file naming and (de)serialization helpers.
pub mod proof;
/// Top-level proving pipeline entry point.
pub mod prove;
/// RISC Zero proving backends: local zkVM, Bonsai, and Boundless.
pub mod risczero;
/// Custom RPC logic
pub mod rpc;
/// Proving worker pool and workload decomposition logic.
pub mod tasks;

/// Control-flow outcomes of a proving attempt.
///
/// Only [ProvingError::OtherError] and [ProvingError::ProvingTimeout] are hard failures. The
/// other variants steer the orchestrator: they report why no proof was produced and carry the
/// traces already collected, so the workload can be split or skipped without repeating work.
#[derive(Debug, thiserror::Error)]
pub enum ProvingError {
    /// The workload was covered by this many execution-only proofs, skipping derivation.
    #[error("DerivationProofError error: execution proofs {0}")]
    SkippingDerivation(usize),

    /// The native run completed without seeking a proof, reporting the witness it collected.
    #[error("NotSeekingProof error: preloaded {preloaded_size} streamed {streamed_size}")]
    NotSeekingProof {
        /// Byte size of the preloaded witness portion.
        preloaded_size: usize,
        /// Byte size of the streamed witness portion.
        streamed_size: usize,
        /// Execution traces collected during the run.
        executions: Vec<Vec<Execution>>,
        /// Partial execution traces collected during the run.
        #[cfg(feature = "experimental")]
        partials: Vec<Vec<PartialExecution>>,
        /// Derivation pipeline snapshot the run started from, if any.
        derivation_cache: Box<Option<CachedDriver>>,
        /// Channel over which the resulting pipeline snapshot is signaled.
        derivation_trace: Option<Sender<CachedDriver>>,
        /// Digest of the resulting pipeline snapshot.
        derivation_trace_hash: B256,
    },

    /// A proof was dispatched, but awaiting its completion was skipped.
    #[error("NotAwaitingProof error")]
    NotAwaitingProof,

    /// More blocks were executed than the configured per-proof limit.
    #[error("BlockCountError error: count {count} limit {limit}")]
    BlockCountError {
        /// Number of blocks executed.
        count: usize,
        /// Configured per-proof block limit.
        limit: usize,
        /// Execution traces collected during the run.
        executions: Vec<Vec<Execution>>,
        /// Partial execution traces collected during the run.
        #[cfg(feature = "experimental")]
        partials: Vec<Vec<PartialExecution>>,
        /// Derivation pipeline snapshot the run started from, if any.
        derivation_cache: Box<Option<CachedDriver>>,
        /// Channel over which the resulting pipeline snapshot is signaled.
        derivation_trace: Option<Sender<CachedDriver>>,
    },

    /// The collected witness exceeds the configured size limit.
    #[error(
        "WitnessSizeError error: preloaded {preloaded_size} streamed {streamed_size} limit {limit}"
    )]
    WitnessSizeError {
        /// Byte size of the preloaded witness portion.
        preloaded_size: usize,
        /// Byte size of the streamed witness portion.
        streamed_size: usize,
        /// Configured witness size limit.
        limit: usize,
        /// Execution traces collected during the run.
        executions: Vec<Vec<Execution>>,
        /// Partial execution traces collected during the run.
        #[cfg(feature = "experimental")]
        partials: Vec<Vec<PartialExecution>>,
        /// Derivation pipeline snapshot the run started from, if any.
        derivation_cache: Box<Option<CachedDriver>>,
        /// Channel over which the resulting pipeline snapshot is signaled.
        derivation_trace: Option<Sender<CachedDriver>>,
    },

    /// Proof computation timed out.
    #[error("ProvingTimeout error")]
    ProvingTimeout,

    /// Unrecoverable failure.
    #[error("OtherError error: {0:?}")]
    OtherError(anyhow::Error),
}

impl ProvingError {
    /// Replaces the carried derivation cache on the splittable variants; others pass through.
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

/// Returns the current UNIX timestamp in seconds.
pub fn current_time() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

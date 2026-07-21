// Copyright 2025 Boundless Foundation, Inc.
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

//! Chain synchronization layer shared by the Kailua proposer, validator, and RPC services.
//!
//! The [agent::SyncAgent] tracks an on-chain Kailua [deployment], replaying every proposal
//! published through the `DisputeGameFactory` into a local tournament state: each [proposal]
//! is checked against op-node output roots, assessed for correctness, and threaded into its
//! parent's tournament. The remaining modules provide the supporting machinery: RPC
//! [provider]s, resilient calls ([mod@retry], [stall]), transaction publication ([transact]),
//! blob KZG math ([blobs]), and OpenTelemetry reporting ([telemetry]).

#![cfg_attr(not(test), warn(missing_docs))]

/// Stateful synchronization agent maintaining a local view of the proposal tournament.
pub mod agent;
/// Common CLI arguments for synchronizing services.
pub mod args;
/// KZG proof computation for blob field elements.
pub mod blobs;
/// Pointers tracking synchronization progress.
pub mod cursor;
/// On-chain Kailua deployment parameters.
pub mod deployment;
/// Provable faults in proposals.
pub mod fault;
/// On-chain proposal data and correctness assessment.
pub mod proposal;
/// RPC providers for L1, L2, op-node, and beacon chain data.
pub mod provider;
/// Macros for retrying fallible operations with exponential backoff.
pub mod retry;
/// Indefinitely retried contract calls.
pub mod stall;
/// OpenTelemetry initialization and instrumentation macros.
pub mod telemetry;
/// Transaction construction, gas pricing, signing, and publication.
pub mod transact;

/// Game type identifier claimed by Kailua in the `DisputeGameFactory`.
pub const KAILUA_GAME_TYPE: u32 = 1337;

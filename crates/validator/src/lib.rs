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

//! The Kailua validator service: watches on-chain proposals and computes and publishes the
//! ZK proofs that decide their tournaments.
//!
//! Two concurrent tasks communicate over a [channel::DuplexChannel]:
//! [proposals::handle_proposals] follows the deployment through a
//! [kailua_sync::agent::SyncAgent], classifying each new proposal (correct, output fault,
//! or trail fault) and queueing responses, while [requests::handle_proof_requests] turns
//! dispatched requests into FPVM proving jobs executed by a [tasks] worker pool. Completed
//! receipts are posted back on-chain as validity or output fault proofs; trail faults are
//! settled with KZG proofs alone.

#![recursion_limit = "256"]
#![cfg_attr(not(test), warn(missing_docs))]

/// Validator CLI arguments.
pub mod args;
/// Message passing between the proposal follower and the proof generator.
pub mod channel;
/// On-chain proposal monitoring and proof publication.
pub mod proposals;
/// Translation of proposal disputes into proving tasks.
pub mod requests;
/// Execution of queued proving tasks.
pub mod tasks;
/// Service entrypoint.
pub mod validate;

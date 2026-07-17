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

//! The Kailua proposer service: extends the chain of on-chain sequencing proposals and
//! finalizes them once they survive dispute.
//!
//! Each iteration of the [propose::propose] loop synchronizes local tournament state through
//! a [kailua_sync::agent::SyncAgent], resolves at most one finalizable proposal
//! ([resolve::resolve_next_pending_proposal]), and, once enough finalized L2 blocks have
//! accumulated, publishes the next proposal (intermediate output roots included as a blob
//! sidecar) through the `KailuaTreasury`.

#![cfg_attr(not(test), warn(missing_docs))]

/// Proposer CLI arguments.
pub mod args;
/// Read-only queries of on-chain treasury and tournament parameters.
pub mod fetch;
/// The main proposer service loop.
pub mod propose;
/// Proposal finalization via tournament pruning and resolution.
pub mod resolve;

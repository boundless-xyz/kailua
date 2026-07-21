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

#![cfg_attr(not(test), warn(missing_docs))]

//! This crate extends the Kailua proving client with EigenDA data availability via Hokulea.
//!
//! Batch data may be fetched from EigenDA blobs in addition to L1 calldata and EIP-4844 blobs.
//! Before the derivation pipeline trusts an EigenDA blob, its DA certificate is validated
//! in-guest against the L1 cert verifier contract using a Steel EVM call (Canoe).

/// In-guest EigenDA certificate validation via Steel (Canoe).
pub mod canoe;
/// Data source factory that inserts EigenDA into the derivation pipeline.
pub mod da;
/// Entry point for running the stitching client with EigenDA enabled.
pub mod stitching;

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

//! This crate extends the Kailua proving client with Celestia data availability via Hana.
//!
//! Batch data may be fetched from Celestia blobs in addition to L1 calldata and EIP-4844 blobs.
//! Celestia reads are only trusted up to the height attested by the SP1 Blobstream contract on
//! L1, which is proven inside the guest using a Steel EVM call anchored to the proposal's L1 head.

/// Data source factory that inserts Celestia into the derivation pipeline.
pub mod da;
/// Blobstream-bounded Celestia blob provider.
pub mod provider;
/// Entry point for running the stitching client with Celestia DA enabled.
pub mod stitching;

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

use crate::rkyv::primitives::B256Def;
use alloy_primitives::B256;
use risc0_zkvm::sha::Digestible;
use risc0_zkvm::Digest;

pub mod derivation;
pub mod execution;
pub mod proposal;

#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    serde::Serialize,
    serde::Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub struct Precondition {
    /// Blob of proposed intermediate outputs whose publication is a precondition
    #[rkyv(with = B256Def)]
    pub proposal_blobs: B256,
    /// Trace of executed blocks whose derivation is a precondition
    #[rkyv(with = B256Def)]
    pub execution_trace: B256,
    /// Cached derivation pipeline whose provability is a precondition
    #[rkyv(with = B256Def)]
    pub derivation_cache: B256,
    /// Derivation pipeline trace whose continuity is a precondition
    #[rkyv(with = B256Def)]
    pub derivation_trace: B256,
}

impl Precondition {
    pub fn execution(mut self, execution_trace: B256) -> Self {
        self.execution_trace = execution_trace;
        self
    }

    pub fn derivation(mut self, derivation_cache: B256, derivation_trace: B256) -> Self {
        self.derivation_cache = derivation_cache;
        self.derivation_trace = derivation_trace;
        self
    }

    pub fn proposal(mut self, proposal_blobs: B256) -> Self {
        self.proposal_blobs = proposal_blobs;
        self
    }
}

impl Digestible for Precondition {
    fn digest(&self) -> Digest {
        // Execution-only precondition
        if !self.execution_trace.is_zero() {
            assert!(self.proposal_blobs.is_zero());
            assert!(self.derivation_cache.is_zero());
            assert!(self.derivation_trace.is_zero());
            return Digest::from_bytes(self.execution_trace.0);
        }
        // Combined proposal/derivation precondition
        Digest::from_bytes(
            combine_precondition_hashes(
                merge_precondition_hashes(self.derivation_cache, self.derivation_trace),
                self.proposal_blobs,
            )
            .0,
        )
    }
}

/// Combines (derivation/blob) precondition hashes
pub fn combine_precondition_hashes(left: B256, right: B256) -> B256 {
    match (left, right) {
        (B256::ZERO, B256::ZERO) => B256::ZERO,
        (a, B256::ZERO) => a,
        (B256::ZERO, b) => b,
        (a, b) => B256::new([a.0, b.0].concat().digest().into()),
    }
}

/// Merges (cache/trace) precondition hashes
pub fn merge_precondition_hashes(left: B256, right: B256) -> B256 {
    match (left, right) {
        (B256::ZERO, B256::ZERO) => B256::ZERO,
        (a, b) => B256::new([a.0, b.0].concat().digest().into()),
    }
}

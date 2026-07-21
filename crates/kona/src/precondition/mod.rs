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

use crate::rkyv::primitives::B256Def;
use alloy_primitives::B256;
use risc0_zkvm::Digest;
use risc0_zkvm::sha::Digestible;

/// Canonical encodings and digests for cached derivation pipeline state.
pub mod derivation;
/// Validation of partial block execution (chunk) preconditions.
#[cfg(feature = "experimental")]
pub mod evm;
/// Hashes committing to standalone block execution traces.
pub mod execution;
/// Loading and validation of proposal blob preconditions for validity proofs.
pub mod proposal;

#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    Eq,
    PartialEq,
    serde::Serialize,
    serde::Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
/// The set of validation conditions a proof commits to beyond its boot claim, digested into the
/// journal's precondition hash.
///
/// At most one flavor is populated: a partial-execution (chunk) trace, an execution-only trace,
/// or a combination of proposal blobs and derivation cache/trace conditions. The digest
/// implementation enforces this exclusivity and collapses the populated fields into one hash,
/// with a lone condition hash digesting to itself.
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
    /// Partial execution trace whose transition is a precondition
    #[rkyv(with = B256Def)]
    pub partial_executions: B256,
}

impl Precondition {
    /// Sets the execution trace condition.
    pub fn execution(mut self, execution_trace: B256) -> Self {
        self.execution_trace = execution_trace;
        self
    }

    /// Sets the derivation cache and derivation trace conditions.
    pub fn derivation(mut self, derivation_cache: B256, derivation_trace: B256) -> Self {
        self.derivation_cache = derivation_cache;
        self.derivation_trace = derivation_trace;
        self
    }

    /// Sets the proposal blobs condition.
    pub fn proposal(mut self, proposal_blobs: B256) -> Self {
        self.proposal_blobs = proposal_blobs;
        self
    }

    /// Sets the partial (chunk) execution trace condition.
    pub fn partial(mut self, partial_execution: B256) -> Self {
        self.partial_executions = partial_execution;
        self
    }
}

impl Digestible for Precondition {
    fn digest(&self) -> Digest {
        // Chunk-only precondition
        if !self.partial_executions.is_zero() {
            assert!(self.proposal_blobs.is_zero());
            assert!(self.execution_trace.is_zero());
            assert!(self.derivation_cache.is_zero());
            assert!(self.derivation_trace.is_zero());
            return Digest::from_bytes(self.partial_executions.0);
        }
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

/// Combines the (derivation, blob) precondition hashes, passing a lone non-zero hash through
/// unchanged and hashing the concatenation when both are set.
pub fn combine_precondition_hashes(left: B256, right: B256) -> B256 {
    match (left, right) {
        (B256::ZERO, B256::ZERO) => B256::ZERO,
        (a, B256::ZERO) => a,
        (B256::ZERO, b) => b,
        (a, b) => B256::new([a.0, b.0].concat().digest().into()),
    }
}

/// Merges the (cache, trace) derivation condition hashes, hashing the concatenation whenever
/// either is non-zero.
pub fn merge_precondition_hashes(left: B256, right: B256) -> B256 {
    match (left, right) {
        (B256::ZERO, B256::ZERO) => B256::ZERO,
        (a, b) => B256::new([a.0, b.0].concat().digest().into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn non_zero_hash(byte: u8) -> B256 {
        B256::new([byte; 32])
    }

    #[test]
    fn default_is_all_zero() {
        let p = Precondition::default();
        assert!(p.proposal_blobs.is_zero());
        assert!(p.execution_trace.is_zero());
        assert!(p.derivation_cache.is_zero());
        assert!(p.derivation_trace.is_zero());
        assert!(p.partial_executions.is_zero());
    }

    #[test]
    fn chunk_builder_sets_only_chunk_trace() {
        let h = non_zero_hash(0xAA);
        let p = Precondition::default().partial(h);
        assert_eq!(p.partial_executions, h);
        assert!(p.proposal_blobs.is_zero());
        assert!(p.execution_trace.is_zero());
        assert!(p.derivation_cache.is_zero());
        assert!(p.derivation_trace.is_zero());
    }

    #[test]
    fn chunk_only_digest() {
        let h = non_zero_hash(0xBB);
        let p = Precondition::default().partial(h);
        assert_eq!(p.digest(), Digest::from_bytes(h.0));
    }

    #[test]
    fn execution_only_digest_unchanged() {
        let h = non_zero_hash(0xCC);
        let p = Precondition::default().execution(h);
        assert_eq!(p.digest(), Digest::from_bytes(h.0));
    }

    #[test]
    fn derivation_proposal_digest_unchanged() {
        let a = non_zero_hash(0x01);
        let b = non_zero_hash(0x02);
        let c = non_zero_hash(0x03);
        let p = Precondition::default().derivation(a, b).proposal(c);
        let expected = combine_precondition_hashes(merge_precondition_hashes(a, b), c);
        assert_eq!(p.digest(), Digest::from_bytes(expected.0));
    }

    #[test]
    #[should_panic]
    fn chunk_trace_with_execution_trace_panics() {
        let p = Precondition {
            partial_executions: non_zero_hash(0x01),
            execution_trace: non_zero_hash(0x02),
            ..Default::default()
        };
        p.digest();
    }

    #[test]
    #[should_panic]
    fn chunk_trace_with_proposal_blobs_panics() {
        let p = Precondition {
            partial_executions: non_zero_hash(0x01),
            proposal_blobs: non_zero_hash(0x02),
            ..Default::default()
        };
        p.digest();
    }

    #[test]
    #[should_panic]
    fn chunk_trace_with_derivation_cache_panics() {
        let p = Precondition {
            partial_executions: non_zero_hash(0x01),
            derivation_cache: non_zero_hash(0x02),
            ..Default::default()
        };
        p.digest();
    }

    #[test]
    #[should_panic]
    fn chunk_trace_with_derivation_trace_panics() {
        let p = Precondition {
            partial_executions: non_zero_hash(0x01),
            derivation_trace: non_zero_hash(0x02),
            ..Default::default()
        };
        p.digest();
    }

    #[test]
    fn all_zero_digest_is_zero() {
        let p = Precondition::default();
        assert_eq!(p.digest(), Digest::from_bytes(B256::ZERO.0));
    }
}

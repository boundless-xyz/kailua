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

/// Oracle wrapper enforcing consistent responses for unauthenticated local keys.
pub mod local;
/// Vector-backed oracle serving witness preimages in expected access order.
pub mod vec;

use alloy_primitives::keccak256;
use kona_preimage::errors::{PreimageOracleError, PreimageOracleResult};
use kona_preimage::{CommsClient, PreimageKey, PreimageKeyType};
use kona_proof::FlushableCache;
use risc0_zkvm::sha::{Impl as SHA2, Sha256};
use std::fmt::Debug;

/// Returns true if preimages under this key type must be authenticated against their key.
///
/// `Local` and `GlobalGeneric` keys do not embed a hash of their value, so they cannot be
/// validated; every other key type must be.
pub fn needs_validation(key_type: &PreimageKeyType) -> bool {
    !matches!(
        key_type,
        PreimageKeyType::Local | PreimageKeyType::GlobalGeneric
    )
}

/// Authenticates `value` against `key` by recomputing the key from the data.
///
/// Keccak256 and Sha256 keys are recomputed with their respective hash and compared, returning
/// `InvalidPreimageKey` on mismatch. Local and GlobalGeneric keys carry no hash and pass without
/// checks. Panics on Precompile keys (acceleration unsupported) and Blob keys (never loaded
/// directly; blobs are authenticated through [crate::blobs]).
pub fn validate_preimage(key: &PreimageKey, value: &[u8]) -> PreimageOracleResult<()> {
    let key_type = key.key_type();
    let image = match key_type {
        PreimageKeyType::Keccak256 => Some(keccak256(value).0),
        PreimageKeyType::Sha256 => {
            let x = SHA2::hash_bytes(value);
            Some(x.as_bytes().try_into().unwrap())
        }
        PreimageKeyType::Precompile => {
            unimplemented!("Precompile acceleration is not yet supported.");
        }
        PreimageKeyType::Blob => {
            unreachable!("Blob key types should not be loaded.");
        }
        PreimageKeyType::Local | PreimageKeyType::GlobalGeneric => None,
    };
    if let Some(image) = image {
        if key != &PreimageKey::new(image, key_type) {
            return Err(PreimageOracleError::InvalidPreimageKey);
        }
    }
    Ok(())
}

/// An oracle that serves preimages recorded ahead of time in a host-supplied witness, rather
/// than communicating with a live host.
///
/// The host populates the oracle with `insert_preimage` while running natively, then calls
/// `finalize_preimages` to shape the data for in-guest consumption. The guest must call
/// `validate_preimages` before serving any data, after which every response is trustworthy.
pub trait WitnessOracle: CommsClient + FlushableCache + Send + Sync + Debug + Default {
    /// Returns the number of preimages currently held by the oracle.
    fn preimage_count(&self) -> usize;

    /// Authenticates every held preimage against its key, erring on any mismatch.
    fn validate_preimages(&self) -> anyhow::Result<()>;

    /// Adds a preimage to the oracle.
    fn insert_preimage(&mut self, key: PreimageKey, value: Vec<u8>);

    /// Prepares the recorded preimages for consumption, sharding them by `shard_size` and
    /// optionally deduplicating validation work via back-references.
    fn finalize_preimages(&mut self, shard_size: usize, with_validation_cache: bool);
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_validate_preimage() {
        // Test Keccak256
        let key = PreimageKey::new(keccak256(b"test").0, PreimageKeyType::Keccak256);
        let value = b"test";
        assert!(validate_preimage(&key, value).is_ok());

        // Test invalid Keccak256
        let invalid_key = PreimageKey::new(keccak256(b"wrong").0, PreimageKeyType::Keccak256);
        assert!(validate_preimage(&invalid_key, value).is_err());

        // Test Sha256
        let sha_value = b"test";
        let sha_key = PreimageKey::new(
            SHA2::hash_bytes(sha_value).as_bytes().try_into().unwrap(),
            PreimageKeyType::Sha256,
        );
        assert!(validate_preimage(&sha_key, sha_value).is_ok());

        // Test invalid Sha256
        let invalid_sha_key = PreimageKey::new(
            SHA2::hash_bytes(b"wrong").as_bytes().try_into().unwrap(),
            PreimageKeyType::Sha256,
        );
        assert!(validate_preimage(&invalid_sha_key, sha_value).is_err());

        // Test Local (no validation)
        let local_key = PreimageKey::new([0u8; 32], PreimageKeyType::Local);
        assert!(validate_preimage(&local_key, b"any value").is_ok());

        // Test GlobalGeneric (no validation)
        let global_key = PreimageKey::new([0u8; 32], PreimageKeyType::GlobalGeneric);
        assert!(validate_preimage(&global_key, b"any value").is_ok());

        // Test Precompile (should panic)
        let precompile_key = PreimageKey::new([0u8; 32], PreimageKeyType::Precompile);
        let result = std::panic::catch_unwind(|| validate_preimage(&precompile_key, b"test"));
        assert!(result.is_err());

        // Test Blob (should panic)
        let blob_key = PreimageKey::new([0u8; 32], PreimageKeyType::Blob);
        let result = std::panic::catch_unwind(|| validate_preimage(&blob_key, b"test"));
        assert!(result.is_err());
    }

    #[test]
    fn test_needs_validation() {
        // Test all PreimageKeyType variants
        assert!(needs_validation(&PreimageKeyType::Keccak256));
        assert!(needs_validation(&PreimageKeyType::Sha256));
        assert!(needs_validation(&PreimageKeyType::Precompile));
        assert!(needs_validation(&PreimageKeyType::Blob));
        assert!(!needs_validation(&PreimageKeyType::Local));
        assert!(!needs_validation(&PreimageKeyType::GlobalGeneric));
    }
}

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

use crate::client::log;
use crate::oracle::WitnessOracle;
use crate::oracle::{needs_validation, validate_preimage};
use crate::rkyv::vec::PreimageVecStoreRkyv;
use alloy_primitives::map::HashMap;
use anyhow::bail;
use async_trait::async_trait;
use kona_preimage::errors::PreimageOracleResult;
use kona_preimage::{HintWriterClient, PreimageKey, PreimageOracleClient};
use kona_proof::FlushableCache;
use lazy_static::lazy_static;
use std::collections::VecDeque;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex};
use tracing::info;

/// A keyed preimage, optionally pointing at the `(shard, index)` position of an earlier
/// occurrence of the same key whose validation covers this duplicate.
pub type IndexedPreimage = (PreimageKey, Vec<u8>, Option<(usize, usize)>);
/// One shard of preimages.
pub type PreimageVecEntry = Vec<IndexedPreimage>;
/// The shared, lock-guarded shard store backing a [VecOracle], holding preimages in reverse of
/// the expected access order.
pub type PreimageVecStore = Arc<Mutex<Vec<PreimageVecEntry>>>;

/// A [WitnessOracle] serving preimages from an in-memory sharded vector arranged in the expected
/// access order.
///
/// Preimages are consumed as they are served; reads arriving out of the expected order are parked
/// in a temporary queue until their key comes up. On zkVM targets, an exhausted store streams
/// further shards from the host, validating each on arrival.
#[derive(Clone, Debug, Default, rkyv::Serialize, rkyv::Archive, rkyv::Deserialize)]
pub struct VecOracle {
    /// The sharded preimage store, in reverse of the expected access order.
    #[rkyv(with = PreimageVecStoreRkyv)]
    pub preimages: PreimageVecStore,
}

impl VecOracle {
    /// Clones the oracle with an independent copy of its `Arc`-shared preimage store, so that
    /// consuming preimages from one copy does not drain the other.
    pub fn deep_clone(&self) -> Self {
        let mut cloned_with_arc = self.clone();
        cloned_with_arc.preimages = Arc::new(Mutex::new(self.preimages.lock().unwrap().clone()));
        cloned_with_arc
    }

    /// Authenticates every preimage in `preimages` against its key.
    ///
    /// An entry carrying a back-reference is instead compared byte-for-byte to the earlier
    /// occurrence it points at, which must strictly precede it in storage order; this makes
    /// duplicate keys cost a comparison instead of a hash. Unauthenticated key types are skipped.
    pub fn validate(preimages: &[PreimageVecEntry]) -> anyhow::Result<()> {
        for (e, entry) in preimages.iter().enumerate() {
            for (p, (key, value, prev)) in entry.iter().enumerate() {
                if !needs_validation(&key.key_type()) {
                    continue;
                } else if let Some((i, j)) = prev {
                    if e < *i {
                        bail!("Attempted to validate preimage against future vec entry.");
                    } else if e == *i && p <= *j {
                        bail!(
                            "Attempted to validate preimage against future preimage in vec entry."
                        );
                    } else if key != &preimages[*i][*j].0 {
                        bail!("Cached preimage key comparison failed");
                    } else if value != &preimages[*i][*j].1 {
                        bail!("Cached preimage value comparison failed");
                    } else {
                        continue;
                    }
                }
                validate_preimage(key, value)?;
            }
        }
        Ok(())
    }
}

impl WitnessOracle for VecOracle {
    fn preimage_count(&self) -> usize {
        self.preimages.lock().unwrap().iter().map(Vec::len).sum()
    }

    fn validate_preimages(&self) -> anyhow::Result<()> {
        let preimages = self.preimages.lock().unwrap();
        Self::validate(preimages.deref())
    }

    /// Validates the pair and appends it to the last shard. Panics on an invalid preimage.
    fn insert_preimage(&mut self, key: PreimageKey, value: Vec<u8>) {
        validate_preimage(&key, &value).expect("Attempted to save invalid preimage");
        let mut preimages = self.preimages.lock().unwrap();
        if preimages.is_empty() {
            preimages.push(Vec::new());
        }
        preimages.last_mut().unwrap().push((key, value, None));
    }

    /// Validates all preimages, then flattens them into shards of at most `shard_size` total
    /// value bytes each, stored in reverse of the recorded access order for consumption by pops.
    /// When `with_validation_ptrs` is set, every repeated key is pointed back at its previous
    /// occurrence so validation can compare bytes instead of re-hashing. Panics if validation
    /// fails.
    fn finalize_preimages(&mut self, shard_size: usize, with_validation_ptrs: bool) {
        self.validate_preimages()
            .expect("Failed to validate preimages during finalization");
        let mut preimages = self.preimages.lock().unwrap();
        // flatten and sort
        let mut flat_vec = core::mem::take(preimages.deref_mut())
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        info!(
            "Finalizing {} preimages with shard size {shard_size} and validation ptrs {with_validation_ptrs}",
            flat_vec.len()
        );
        // sort by expected access
        flat_vec.reverse();
        // shard vectors by size limit
        let mut sharded_vec = vec![vec![]];
        let mut last_shard_size = 0;
        for value in flat_vec {
            if value.1.len() + last_shard_size > shard_size && last_shard_size > 0 {
                sharded_vec.push(vec![]);
                last_shard_size = 0;
            }
            last_shard_size += value.1.len();
            sharded_vec.last_mut().unwrap().push(value);
        }
        let _ = core::mem::replace(preimages.deref_mut(), sharded_vec);
        // add validation pointers
        if !with_validation_ptrs {
            return;
        }
        let mut cache: HashMap<PreimageKey, (usize, usize)> =
            HashMap::with_capacity_and_hasher(preimages.len(), Default::default());
        for (i, entry) in preimages.iter_mut().enumerate() {
            for (j, (key, _, pointer)) in entry.iter_mut().enumerate() {
                if !needs_validation(&key.key_type()) {
                    continue;
                } else if let Some(prev) = cache.insert(*key, (i, j)) {
                    pointer.replace(prev);
                }
            }
        }
    }
}

impl FlushableCache for VecOracle {
    fn flush(&self) {}
}

/// A queue of preimages popped while seeking another key, awaiting reinsertion into the store.
pub type PreimageQueue = VecDeque<IndexedPreimage>;

lazy_static! {
    /// An object used for temporary storage of out-of-order preimages accessed randomly.
    static ref QUEUE: Arc<Mutex<PreimageQueue>> = Default::default();
}

#[async_trait]
impl PreimageOracleClient for VecOracle {
    /// Serves the preimage for `key` by popping entries off the store in expected access order.
    ///
    /// Entries popped while seeking `key` are parked in a temporary queue and restored to the
    /// current shard once the sought key is found. If the store runs out, zkVM targets stream
    /// and validate the next shard from the host; other targets panic on exhaustion.
    async fn get(&self, key: PreimageKey) -> PreimageOracleResult<Vec<u8>> {
        let mut preimages = self.preimages.lock().unwrap();
        let mut queue = QUEUE.lock().unwrap_or_else(|poisoned| {
            // restore the empty-queue invariant when needed (relevant for concurrent testing only)
            QUEUE.clear_poison();
            let mut queue = poisoned.into_inner();
            queue.clear();
            queue
        });
        // handle variations in memory access operations due to hashmap usages
        loop {
            if preimages.is_empty() {
                #[cfg(target_os = "zkvm")]
                {
                    crate::client::log("DESERIALIZE STREAMED SHARD");
                    preimages.push(read_shard());
                    Self::validate(preimages.as_ref())
                        .expect("Failed to validate streamed preimages");
                    crate::client::log("STREAMED SHARD VALIDATED");
                }
                #[cfg(not(target_os = "zkvm"))]
                panic!(
                    "Exhausted VecOracle seeking {key} ({} queued preimages)",
                    queue.len()
                )
            }

            let entry = preimages.last_mut().unwrap();
            while let Some((last_key, value, _)) = entry.pop() {
                if key == last_key {
                    if !queue.is_empty() {
                        log(&format!("TEMP ELEMENTS: {}", queue.len()));
                        entry.extend(core::mem::take(queue.deref_mut()));
                    }

                    return Ok(value);
                }
                // keep entry in queue for later use, pointer is no longer necessary
                queue.push_front((last_key, value, None));
            }
            preimages.pop();
        }
    }

    /// Like [Self::get], but copies the value into `buf`, panicking if the lengths differ.
    async fn get_exact(&self, key: PreimageKey, buf: &mut [u8]) -> PreimageOracleResult<()> {
        let value = self.get(key).await?;
        buf.copy_from_slice(value.as_ref());
        Ok(())
    }
}

#[async_trait]
impl HintWriterClient for VecOracle {
    /// No-op: the witness already contains every preimage the client will request.
    async fn write(&self, _hint: &str) -> PreimageOracleResult<()> {
        Ok(())
    }
}

/// Reads the next rkyv-serialized preimage shard from the host over the zkVM input stream,
/// panicking if deserialization fails. The shard is untrusted until validated.
#[cfg(target_os = "zkvm")]
pub fn read_shard() -> PreimageVecEntry {
    let shard_data = risc0_zkvm::guest::env::read_frame();
    rkyv::from_bytes::<PreimageVecEntry, rkyv::rancor::Error>(&shard_data)
        .expect("Failed to deserialize shard")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod tests {
    use super::*;
    use alloy_primitives::keccak256;
    use kona_preimage::PreimageKeyType;
    use kona_proof::block_on;
    use risc0_zkvm::sha::{Impl as SHA2, Sha256};
    use rkyv::rancor::Error;
    use std::collections::HashSet;

    pub fn prepare_vec_oracle(value_count: usize, copies: usize) -> (VecOracle, Vec<Vec<u8>>) {
        let mut oracle = VecOracle::default();
        assert_eq!(oracle.preimage_count(), 0);

        let values = (0..value_count)
            .map(|i| format!("{i} test {i} value {i}").as_bytes().to_vec())
            .collect::<Vec<_>>();
        // insert sha3 keys
        for value in &values {
            let sha3_key = PreimageKey::new_keccak256(keccak256(value).0);
            for _ in 0..copies {
                oracle.insert_preimage(sha3_key, value.clone());
            }
        }
        oracle.validate_preimages().unwrap();
        assert_eq!(oracle.preimage_count(), values.len() * copies);
        // insert sha2 keys
        for value in &values {
            let sha2_key = PreimageKey::new(
                SHA2::hash_bytes(value).as_bytes().try_into().unwrap(),
                PreimageKeyType::Sha256,
            );
            for _ in 0..copies {
                oracle.insert_preimage(sha2_key, value.clone());
            }
        }
        oracle.validate_preimages().unwrap();
        assert_eq!(oracle.preimage_count(), values.len() * copies * 2);

        (oracle, values)
    }

    pub async fn exhaust_vec_oracle(copies: usize, oracle: VecOracle, values: Vec<Vec<u8>>) {
        let initial_size = oracle.preimage_count();
        for value in values.iter().rev() {
            let sha3_key = PreimageKey::new_keccak256(keccak256(value).0);
            let sha2_key = PreimageKey::new(
                SHA2::hash_bytes(value).as_bytes().try_into().unwrap(),
                PreimageKeyType::Sha256,
            );
            for _ in 0..copies {
                let mut sha3_val = vec![0u8; value.len()];
                oracle.get_exact(sha3_key, &mut sha3_val).await.unwrap();
                let mut sha2_val = vec![0u8; value.len()];
                oracle.get_exact(sha2_key, &mut sha2_val).await.unwrap();
                assert_eq!(sha3_val, sha2_val);
            }
        }
        // ensure exhaustion
        assert_eq!(
            oracle.preimage_count(),
            initial_size - 2 * copies * values.len()
        );
    }

    #[tokio::test]
    async fn test_deep_clone() {
        let (mut oracle, values) = prepare_vec_oracle(1024, 3);
        oracle.insert_preimage(
            PreimageKey::new([0xff; 32], PreimageKeyType::Local),
            vec![0xff; 32],
        );
        oracle.finalize_preimages(1, true);
        oracle.validate_preimages().unwrap();
        // assert initial equivalence
        let size = oracle.preimage_count();
        let cloned = oracle.deep_clone();
        assert_eq!(size, cloned.preimage_count());
        // regular cloning vs deep cloning
        exhaust_vec_oracle(3, oracle.clone(), values).await;
        assert_eq!(oracle.preimage_count(), 1);
        assert_eq!(size, cloned.preimage_count());
    }

    #[tokio::test]
    async fn test_vec_oracle_sharded() {
        let (mut oracle, values) = prepare_vec_oracle(1024, 1);
        // one key per shard
        oracle.finalize_preimages(1, true);
        oracle.validate_preimages().unwrap();
        // serde
        let oracle = rkyv::from_bytes::<VecOracle, Error>(
            rkyv::to_bytes::<Error>(&oracle).unwrap().as_ref(),
        )
        .unwrap();
        // validate
        {
            let preimage_vecs = oracle.preimages.lock().unwrap();
            assert_eq!(preimage_vecs.len(), values.len() * 2);
            for preimages in preimage_vecs.iter() {
                assert_eq!(preimages.len(), 1);
                for preimage in preimages.iter() {
                    assert_eq!(preimage.2, None);
                }
            }
        }
        // retrieve keys
        exhaust_vec_oracle(1, oracle, values).await;
    }

    #[tokio::test]
    async fn test_vec_oracle_unsharded() {
        let (mut oracle, values) = prepare_vec_oracle(1024, 1);
        // one shard for all keys
        oracle.finalize_preimages(usize::MAX, true);
        oracle.validate_preimages().unwrap();
        // serde
        let oracle = rkyv::from_bytes::<VecOracle, Error>(
            rkyv::to_bytes::<Error>(&oracle).unwrap().as_ref(),
        )
        .unwrap();
        // validate
        {
            let preimage_vecs = oracle.preimages.lock().unwrap();
            assert_eq!(preimage_vecs.len(), 1);
            for preimages in preimage_vecs.iter() {
                assert_eq!(preimages.len(), values.len() * 2);
                for preimage in preimages.iter() {
                    assert_eq!(preimage.2, None);
                }
            }
        }
        // retrieve keys
        exhaust_vec_oracle(1, oracle, values).await;
    }

    #[tokio::test]
    async fn test_vec_oracle_duplicates_sharded() {
        let (mut oracle, values) = prepare_vec_oracle(1024, 2);
        // one key per shard
        oracle.finalize_preimages(1, true);
        oracle.validate_preimages().unwrap();
        // serde
        let oracle = rkyv::from_bytes::<VecOracle, Error>(
            rkyv::to_bytes::<Error>(&oracle).unwrap().as_ref(),
        )
        .unwrap();
        // validate
        {
            let preimage_vecs = oracle.preimages.lock().unwrap();
            assert_eq!(preimage_vecs.len(), values.len() * 2 * 2);
            let mut seen_keys = HashSet::new();
            for preimages in preimage_vecs.iter() {
                assert_eq!(preimages.len(), 1);
                for preimage in preimages.iter() {
                    if seen_keys.contains(&preimage.0) {
                        let ptr = preimage.2.unwrap();
                        assert_eq!(&preimage_vecs[ptr.0][ptr.1].0, &preimage.0);
                    } else {
                        assert!(preimage.2.is_none());
                        seen_keys.insert(preimage.0);
                    }
                }
            }
        }
        // retrieve keys
        exhaust_vec_oracle(2, oracle, values).await;
    }

    #[tokio::test]
    async fn test_vec_oracle_duplicates_unsharded() {
        let (mut oracle, values) = prepare_vec_oracle(1024, 2);
        // one shard
        oracle.finalize_preimages(usize::MAX, true);
        oracle.validate_preimages().unwrap();
        // serde
        let oracle = rkyv::from_bytes::<VecOracle, Error>(
            rkyv::to_bytes::<Error>(&oracle).unwrap().as_ref(),
        )
        .unwrap();
        // validate
        {
            let preimage_vecs = oracle.preimages.lock().unwrap();
            assert_eq!(preimage_vecs.len(), 1);
            let mut seen_keys = HashSet::new();
            for preimages in preimage_vecs.iter() {
                assert_eq!(preimages.len(), values.len() * 2 * 2);
                for preimage in preimages.iter() {
                    if seen_keys.contains(&preimage.0) {
                        let ptr = preimage.2.unwrap();
                        assert_eq!(&preimage_vecs[ptr.0][ptr.1].0, &preimage.0);
                    } else {
                        assert!(preimage.2.is_none());
                        seen_keys.insert(preimage.0);
                    }
                }
            }
        }
        // retrieve keys
        exhaust_vec_oracle(2, oracle, values).await;
    }

    #[tokio::test]
    async fn test_vec_oracle_duplicates_unsharded_no_cache() {
        let (mut oracle, values) = prepare_vec_oracle(1024, 2);
        // one shard
        oracle.finalize_preimages(usize::MAX, false);
        oracle.validate_preimages().unwrap();
        // serde
        let oracle = rkyv::from_bytes::<VecOracle, Error>(
            rkyv::to_bytes::<Error>(&oracle).unwrap().as_ref(),
        )
        .unwrap();
        // validate
        {
            let preimage_vecs = oracle.preimages.lock().unwrap();
            assert_eq!(preimage_vecs.len(), 1);
            for preimages in preimage_vecs.iter() {
                assert_eq!(preimages.len(), values.len() * 2 * 2);
                for preimage in preimages.iter() {
                    assert!(preimage.2.is_none());
                }
            }
        }
        // retrieve keys
        exhaust_vec_oracle(2, oracle, values).await;
    }

    #[test]
    fn test_vec_oracle_tamper() {
        let (mut oracle, _) = prepare_vec_oracle(1, 4);
        // one key pre shard
        oracle.finalize_preimages(1, true);
        oracle.validate_preimages().unwrap();

        // point first entry to future entry
        {
            let oracle = oracle.deep_clone();
            {
                let mut preimages = oracle.preimages.lock().unwrap();
                let preimage_vec = preimages.first_mut().unwrap();
                let preimage = preimage_vec.first_mut().unwrap();
                preimage.2 = Some((1, 0));
            }
            // fail to validate
            let result = oracle.validate_preimages().unwrap_err();
            assert!(result.to_string().contains("future vec entry"));
        }
        // point first entry to self
        {
            let oracle = oracle.deep_clone();
            {
                let mut preimages = oracle.preimages.lock().unwrap();
                let preimage_vec = preimages.first_mut().unwrap();
                let preimage = preimage_vec.first_mut().unwrap();
                preimage.2 = Some((0, 0));
            }
            // fail to validate
            let result = oracle.validate_preimages().unwrap_err();
            assert!(result.to_string().contains("future preimage"));
        }
        // invalidate key
        {
            let oracle = oracle.deep_clone();
            {
                let mut preimages = oracle.preimages.lock().unwrap();
                let preimage_vec = preimages.first_mut().unwrap();
                let preimage = preimage_vec.first_mut().unwrap();
                preimage.0 = PreimageKey::new([0xff; 32], PreimageKeyType::Local);
            }
            // fail to validate
            let result = oracle.validate_preimages().unwrap_err();
            assert!(result.to_string().contains("key comparison failed"));
        }
        // invalidate value
        {
            let oracle = oracle.deep_clone();
            {
                let mut preimages = oracle.preimages.lock().unwrap();
                let preimage_vec = preimages.last_mut().unwrap();
                let preimage = preimage_vec.first_mut().unwrap();
                preimage.1 = vec![0xff; 32];
            }
            // fail to validate
            let result = oracle.validate_preimages().unwrap_err();
            assert!(result.to_string().contains("value comparison failed"));
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_exhaustion() {
        let (mut oracle, values) = prepare_vec_oracle(1, 1);
        oracle.finalize_preimages(usize::MAX, true);
        oracle.validate_preimages().unwrap();
        // fail to refetch key after exhaustion
        let only_key = oracle
            .preimages
            .lock()
            .unwrap()
            .first()
            .unwrap()
            .first()
            .unwrap()
            .0;
        exhaust_vec_oracle(1, oracle.clone(), values).await;
        assert!(std::panic::catch_unwind(|| block_on(oracle.get(only_key))).is_err());
        // the panic above poisons the per-oracle preimages lock
        assert!(oracle.preimages.is_poisoned());
        // the next lookup recovers the global queue from the poisoned state
        let (mut oracle, values) = prepare_vec_oracle(1, 1);
        oracle.finalize_preimages(usize::MAX, true);
        exhaust_vec_oracle(1, oracle, values).await;
    }

    #[tokio::test]
    async fn test_noop() {
        let oracle = prepare_vec_oracle(0, 0).0;
        oracle.write("noop").await.unwrap();
        oracle.flush();
        assert_eq!(oracle.preimage_count(), 0);
    }
}

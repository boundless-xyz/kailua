// Copyright 2026 RISC Zero, Inc.
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

//! Hint-handler wrapper that delays retries of failing hint fetches.
//!
//! kona's `OnlineHostBackend::get_preimage` retries `fetch_hint` in an unbounded loop with no
//! delay between attempts. When a hint cannot be served (rate-limited RPC, block unknown to the
//! node), that loop re-sends the same request as fast as the network round trip allows and can
//! flood the RPC provider. Sleeping in the error path here paces that loop without requiring
//! kona changes: the loop cannot start the next attempt until `fetch_hint` returns.
//!
//! Errors from a method blocked via `--blocked-rpc-methods` are exempt from the delay: they
//! can never succeed, so kona should fall through to fine-grained hints without pacing. A
//! generic "method not found" is not exempt, since behind a load balancer it can recover on
//! retry (see [is_method_blacklisted]).

use alloy_primitives::{keccak256, B256};
use anyhow::Result;
use async_trait::async_trait;
use kailua_sync::retry::{MAX_DELAY_MS, MIN_DELAY_MS};
use kona_host::{HintHandler, OnlineHostBackendCfg, SharedKeyValueStore};
use kona_proof::Hint;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::marker::PhantomData;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tracing::warn;

/// Consecutive-failure state for a single hint.
struct FailureEntry {
    consecutive: u32,
    last_touch: Instant,
}

/// Failure counts keyed by hint type and data digest.
type FailureMap = HashMap<(u64, B256), FailureEntry>;

/// Shared across every proving session in the process, so concurrent sessions failing on the
/// same hint back off together instead of independently re-firing at full speed.
static FAILURES: LazyLock<Mutex<FailureMap>> = LazyLock::new(Default::default);

/// Entries not touched for this long are dropped once the map grows past [PRUNE_THRESHOLD].
const STALE_AFTER: Duration = Duration::from_secs(600);
const PRUNE_THRESHOLD: usize = 1024;

fn hint_key<T: Hash>(ty: &T, data: &[u8]) -> (u64, B256) {
    let mut hasher = DefaultHasher::new();
    ty.hash(&mut hasher);
    (hasher.finish(), keccak256(data))
}

/// Returns the delay to apply after the nth consecutive failure, doubling from
/// [MIN_DELAY_MS] up to [MAX_DELAY_MS].
fn backoff_delay(consecutive_failures: u32) -> Duration {
    let exponent = consecutive_failures.saturating_sub(1).min(31);
    let millis = MIN_DELAY_MS
        .checked_shl(exponent)
        .unwrap_or(MAX_DELAY_MS)
        .min(MAX_DELAY_MS);
    Duration::from_millis(millis)
}

/// Returns true for an error from a method blocked locally by `--blocked-rpc-methods`.
///
/// A blocked method can never succeed, so callers give up (skip retries and backoff delay)
/// on it. A generic "method not found" from the provider is deliberately not matched: behind
/// a load balancer it can be transient and recover on a later attempt, so it stays on the
/// normal retry path. To fast-path a method a node genuinely lacks, add it to
/// `--blocked-rpc-methods`.
pub(crate) fn is_method_blacklisted(err: &anyhow::Error) -> bool {
    format!("{err:#}").contains(crate::rpc::BLOCKED_METHOD_MARKER)
}

fn record_failure(map: &mut FailureMap, key: (u64, B256), now: Instant) -> (u32, Duration) {
    if map.len() >= PRUNE_THRESHOLD {
        map.retain(|_, entry| now.duration_since(entry.last_touch) < STALE_AFTER);
    }
    let entry = map.entry(key).or_insert(FailureEntry {
        consecutive: 0,
        last_touch: now,
    });
    entry.consecutive = entry.consecutive.saturating_add(1);
    entry.last_touch = now;
    (entry.consecutive, backoff_delay(entry.consecutive))
}

/// Generic hint-handler wrapper that sleeps before returning a hint fetch error, with the sleep
/// doubling on every consecutive failure of the same hint and resetting on success.
#[derive(Debug, Clone, Copy)]
pub struct BackoffWrapper<Inner, Cfg>(PhantomData<(Inner, Cfg)>);

impl<Inner, Cfg> Default for BackoffWrapper<Inner, Cfg> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

#[async_trait]
impl<Inner, Cfg> HintHandler for BackoffWrapper<Inner, Cfg>
where
    Inner: HintHandler<Cfg = Cfg> + Send + Sync + 'static,
    Cfg: OnlineHostBackendCfg + Send + Sync + 'static,
{
    type Cfg = Cfg;

    async fn fetch_hint(
        hint: Hint<<Self::Cfg as OnlineHostBackendCfg>::HintType>,
        cfg: &Self::Cfg,
        providers: &<Self::Cfg as OnlineHostBackendCfg>::Providers,
        kv: SharedKeyValueStore,
    ) -> Result<()> {
        let key = hint_key(&hint.ty, hint.data.as_ref());
        match Inner::fetch_hint(hint, cfg, providers, kv).await {
            Ok(()) => {
                FAILURES
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&key);
                Ok(())
            }
            Err(err) => {
                if is_method_blacklisted(&err) {
                    // Intentionally blocked; retrying cannot help, so return without delay
                    // and let kona fall through to fine-grained hints.
                    return Err(err);
                }
                let (attempts, delay) = record_failure(
                    &mut FAILURES.lock().unwrap_or_else(PoisonError::into_inner),
                    key,
                    Instant::now(),
                );
                warn!(
                    attempts,
                    delay_ms = delay.as_millis() as u64,
                    "Hint fetch failed, delaying next attempt"
                );
                tokio::time::sleep(delay).await;
                Err(err)
            }
        }
    }
}

pub type BackoffFallbackBlobHintHandler = BackoffWrapper<
    crate::hint_handler::FallbackBlobHintHandler,
    kona_host::single::SingleChainHost,
>;

#[cfg(feature = "eigen")]
pub type BackoffFallbackBlobHintHandlerWithEigenDA = BackoffWrapper<
    crate::hint_handler::FallbackBlobHintHandlerWithEigenDA,
    hokulea_host_bin::cfg::SingleChainHostWithEigenDA,
>;

#[cfg(feature = "celestia")]
pub type BackoffFallbackHanaHintHandler = BackoffWrapper<
    crate::hint_handler::FallbackHanaHintHandler,
    hana_host::celestia::CelestiaChainHost,
>;

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Bytes;
    use anyhow::anyhow;
    use kona_host::MemoryKeyValueStore;
    use kona_proof::errors::HintParsingError;
    use std::str::FromStr;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    #[test]
    fn backoff_delay_doubles_and_caps() {
        assert_eq!(backoff_delay(1), Duration::from_millis(MIN_DELAY_MS));
        assert_eq!(backoff_delay(2), Duration::from_millis(MIN_DELAY_MS * 2));
        assert_eq!(backoff_delay(3), Duration::from_millis(MIN_DELAY_MS * 4));
        assert_eq!(backoff_delay(7), Duration::from_millis(MAX_DELAY_MS));
        assert_eq!(backoff_delay(100), Duration::from_millis(MAX_DELAY_MS));
        assert_eq!(backoff_delay(u32::MAX), Duration::from_millis(MAX_DELAY_MS));
    }

    #[test]
    fn record_failure_tracks_hints_independently() {
        let mut map = FailureMap::default();
        let now = Instant::now();
        let key_a = hint_key(&1u8, b"a");
        let key_b = hint_key(&1u8, b"b");

        assert_eq!(record_failure(&mut map, key_a, now).0, 1);
        assert_eq!(record_failure(&mut map, key_a, now).0, 2);
        assert_eq!(record_failure(&mut map, key_b, now).0, 1);

        map.remove(&key_a);
        assert_eq!(record_failure(&mut map, key_a, now).0, 1);
    }

    #[test]
    fn record_failure_prunes_stale_entries() {
        let mut map = FailureMap::default();
        let start = Instant::now();
        for i in 0..PRUNE_THRESHOLD {
            record_failure(&mut map, hint_key(&i, b"stale"), start);
        }
        assert_eq!(map.len(), PRUNE_THRESHOLD);

        // A failure recorded past the staleness window evicts everything older.
        let later = start + STALE_AFTER + Duration::from_secs(1);
        record_failure(&mut map, hint_key(&usize::MAX, b"fresh"), later);
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn hint_key_distinguishes_type_and_data() {
        assert_ne!(hint_key(&1u8, b"data"), hint_key(&2u8, b"data"));
        assert_ne!(hint_key(&1u8, b"data"), hint_key(&1u8, b"other"));
        assert_eq!(hint_key(&1u8, b"data"), hint_key(&1u8, b"data"));
    }

    #[test]
    fn method_blacklisted_detection() {
        // The middleware marker is classified as blacklisted, through anyhow context chains.
        let blocked = anyhow!(
            "error code -32601: the method debug_executePayload is {}",
            crate::rpc::BLOCKED_METHOD_MARKER
        );
        assert!(is_method_blacklisted(&blocked));
        assert!(is_method_blacklisted(
            &blocked.context("debug_executePayload failed")
        ));
        // A generic method-not-found is NOT blacklisted: it may recover across a
        // load-balanced pool, so retry loops must keep trying.
        assert!(!is_method_blacklisted(&anyhow!(
            "error code -32601: the method debug_executePayload does not exist/is not available"
        )));
        assert!(!is_method_blacklisted(&anyhow!("HTTP error 429")));
    }

    #[derive(Clone, PartialEq, Eq, Hash, Debug)]
    struct TestHint;

    impl FromStr for TestHint {
        type Err = HintParsingError;

        fn from_str(_: &str) -> Result<Self, Self::Err> {
            Ok(Self)
        }
    }

    struct TestCfg;

    impl OnlineHostBackendCfg for TestCfg {
        type HintType = TestHint;
        type Providers = ();
    }

    struct FailingHandler;

    #[async_trait]
    impl HintHandler for FailingHandler {
        type Cfg = TestCfg;

        async fn fetch_hint(
            _hint: Hint<TestHint>,
            _cfg: &TestCfg,
            _providers: &(),
            _kv: SharedKeyValueStore,
        ) -> Result<()> {
            Err(anyhow!("fetch failed"))
        }
    }

    struct BlacklistedHandler;

    #[async_trait]
    impl HintHandler for BlacklistedHandler {
        type Cfg = TestCfg;

        async fn fetch_hint(
            _hint: Hint<TestHint>,
            _cfg: &TestCfg,
            _providers: &(),
            _kv: SharedKeyValueStore,
        ) -> Result<()> {
            Err(anyhow!(
                "error code -32601: the method debug_executePayload is {}",
                crate::rpc::BLOCKED_METHOD_MARKER
            ))
        }
    }

    struct SucceedingHandler;

    #[async_trait]
    impl HintHandler for SucceedingHandler {
        type Cfg = TestCfg;

        async fn fetch_hint(
            _hint: Hint<TestHint>,
            _cfg: &TestCfg,
            _providers: &(),
            _kv: SharedKeyValueStore,
        ) -> Result<()> {
            Ok(())
        }
    }

    fn test_hint(data: &'static [u8]) -> Hint<TestHint> {
        Hint {
            ty: TestHint,
            data: Bytes::from_static(data),
        }
    }

    fn test_kv() -> SharedKeyValueStore {
        Arc::new(RwLock::new(MemoryKeyValueStore::default()))
    }

    #[tokio::test]
    async fn blacklisted_method_passes_through_undelayed() {
        let data: &[u8] = b"blacklisted_method_passes_through_undelayed";
        let key = hint_key(&TestHint, data);

        let started = Instant::now();
        BackoffWrapper::<BlacklistedHandler, TestCfg>::fetch_hint(
            test_hint(data),
            &TestCfg,
            &(),
            test_kv(),
        )
        .await
        .unwrap_err();
        assert!(started.elapsed() < Duration::from_millis(MIN_DELAY_MS));
        assert!(!FAILURES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(&key));
    }

    #[tokio::test]
    async fn failure_delays_and_success_resets() {
        let data: &[u8] = b"failure_delays_and_success_resets";
        let key = hint_key(&TestHint, data);

        let started = Instant::now();
        BackoffWrapper::<FailingHandler, TestCfg>::fetch_hint(
            test_hint(data),
            &TestCfg,
            &(),
            test_kv(),
        )
        .await
        .unwrap_err();
        assert!(started.elapsed() >= Duration::from_millis(MIN_DELAY_MS));
        assert_eq!(
            FAILURES
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&key)
                .map(|entry| entry.consecutive),
            Some(1)
        );

        BackoffWrapper::<SucceedingHandler, TestCfg>::fetch_hint(
            test_hint(data),
            &TestCfg,
            &(),
            test_kv(),
        )
        .await
        .unwrap();
        assert!(!FAILURES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(&key));
    }
}

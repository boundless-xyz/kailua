## Why

The KailuaVerifier contract (v1.2.0) added a `numDelayedPermits` parameter to three functions (`countExpiredPermits`, `acquireFaultProofPermit`, `releaseFaultProofPermit`). The Rust code in `agent.rs` still uses the old signatures, causing 6 compilation errors that block the entire `kailua-validator` build.

## What Changes

- Add missing `numDelayedPermits` argument to `countExpiredPermits` calls (2 call sites)
- Add missing `numDelayedPermits` argument to `acquireFaultProofPermit` call (1 call site)
- Add missing `numDelayedPermits` argument to `releaseFaultProofPermit` call (1 call site)
- Fix return tuple indexing for `countExpiredPermits`: `_1` is now `numDelayedPermits`, `_2` is `expiredCollateral` (U256), `_3` is `numActivePermits`
- Fix `num_issued_permits` calculation to include delayed permits
- Fix payout calculation to use correct tuple indices

## Capabilities

### New Capabilities
- `delayed-permits-abi`: Update agent.rs to match KailuaVerifier v1.2.0 ABI with numDelayedPermits parameter

### Modified Capabilities

## Impact

- `crates/sync/src/agent.rs` — 6 edits across `acquire_fp_permit` and `release_fp_permit` functions

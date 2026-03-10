## Why

The KailuaVerifier contract now supports a `PERMIT_DELAY` parameter that prevents a fault proof permit from taking effect immediately upon acquisition. If the validator submits a fault proof while its permit is still in the DELAYED state, it misses out on the reward share (`expiredCollateral / numActivePermits`). The validator needs to wait for permit activation before submitting proofs to maximize economic return.

## What Changes

- Cache `PERMIT_DURATION` and `PERMIT_DELAY` immutable values in `SyncDeployment` at startup
- Gate fault proof submission (both output and trail) on permit activation time
- Use L1 block timestamp (not local clock) for timing decisions, matching the contract's `block.timestamp`
- If the validator's permit index is 0, assume it's the sole permit holder and submit immediately
- If the permit index is > 0, wait until the permit is activated before submitting

## Capabilities

### New Capabilities
- `permit-activation-gate`: Gate fault proof submission on permit activation timing, delaying on-chain submission when the validator's permit is not yet active and other permits exist

### Modified Capabilities

## Impact

- `crates/sync/src/deployment.rs` — New fields for cached permit timing constants
- `crates/validator/src/proposals/receipts.rs` — Submission gate for output fault proofs
- `crates/validator/src/proposals/trails.rs` — Submission gate for trail fault proofs

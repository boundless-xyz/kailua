## Context

The KailuaVerifier contract v1.2.0 introduced delayed permit activations via a new `numDelayedPermits` parameter. Three contract functions changed their signatures, and `countExpiredPermits` now returns a 4-tuple `(numExpiredPermits, numDelayedPermits, expiredCollateral, numActivePermits)` where `numActivePermits = totalPermits - numExpiredPermits - numDelayedPermits`.

The Rust code in `agent.rs` calls these functions with the old signatures and destructures the return tuple using stale indices.

## Goals / Non-Goals

**Goals:**
- Fix all 6 compilation errors in `agent.rs`
- Correctly pass `numDelayedPermits` to all three contract functions
- Correctly destructure the updated return tuple

**Non-Goals:**
- Changing permit acquisition or release strategy
- Optimizing the delayed permits hint (passing `0` and letting the contract compute is sufficient)

## Decisions

### 1. Pass `0` as numDelayedPermits hint
**Decision:** Pass `0` for the `numDelayedPermits` parameter in `countExpiredPermits` and `acquireFaultProofPermit` calls.
**Rationale:** Matches the existing pattern — `numExpiredPermits` is also passed as `0` and the contract auto-corrects. The contract scans from the hint value, so `0` is always safe.

### 2. Forward contract-computed numDelayedPermits to releaseFaultProofPermit
**Decision:** Use `permit_counts._1` (the numDelayedPermits returned by `countExpiredPermits`) when calling `releaseFaultProofPermit`.
**Rationale:** The release function needs the accurate count as of proof submission time. The contract already computed this in the preceding `countExpiredPermits` call.

### 3. Include delayed permits in total issued count
**Decision:** `num_issued_permits = num_expired + num_delayed + num_active` (all three categories).
**Rationale:** The availability check `num_issued_permits > 2 * num_expired_permits` needs the true total. Previously, delayed permits didn't exist so `total = expired + active` was correct. Now delayed permits are a third category.

## Risks / Trade-offs

- **[Risk] None significant** — This is a mechanical ABI alignment fix with no behavioral changes beyond correctly accounting for delayed permits.

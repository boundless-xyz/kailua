## 1. Fix countExpiredPermits in acquire_fp_permit

- [x] 1.1 Add `0` as 3rd argument (numDelayedPermits) to `countExpiredPermits` call at line 989
- [x] 1.2 Fix return indexing: change `permit_counts._2` to `permit_counts._3` for numActivePermits at line 997
- [x] 1.3 Include delayed permits in total: `num_issued_permits = num_expired_permits + permit_counts._1 + num_active_permits` at line 998

## 2. Fix acquireFaultProofPermit call

- [x] 2.1 Add `0` as 4th argument (numDelayedPermits) between `num_expired_permits` and `payout_recipient` at line 1014

## 3. Fix countExpiredPermits in release_fp_permit

- [x] 3.1 Add `0` as 3rd argument (numDelayedPermits) to `countExpiredPermits` call at line 1128

## 4. Fix payout calculation and releaseFaultProofPermit call

- [x] 4.1 Fix payout calculation at line 1137: change to `permit_counts._2 / U256::from(permit_counts._3)`
- [x] 4.2 Add `permit_counts._1` as 4th argument (numDelayedPermits) to `releaseFaultProofPermit` between `permit_counts._0` and `index` at line 1169

## 5. Verification

- [x] 5.1 Verify the project compiles with `cargo build -p kailua-validator`

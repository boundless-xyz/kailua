## ADDED Requirements

### Requirement: countExpiredPermits includes numDelayedPermits argument
The system SHALL pass `numDelayedPermits` as the third argument to `KailuaVerifier::countExpiredPermits()`, matching the contract's 4-parameter signature `(proposalKey, numExpiredPermits, numDelayedPermits, timestamp)`.

#### Scenario: countExpiredPermits called during permit acquisition
- **WHEN** `acquire_fp_permit` calls `countExpiredPermits`
- **THEN** the call SHALL include `0` as the `numDelayedPermits` argument (3rd position)

#### Scenario: countExpiredPermits called during permit release
- **WHEN** `release_fp_permit` calls `countExpiredPermits`
- **THEN** the call SHALL include `0` as the `numDelayedPermits` argument (3rd position)

### Requirement: acquireFaultProofPermit includes numDelayedPermits argument
The system SHALL pass `numDelayedPermits` as the fourth argument to `KailuaVerifier::acquireFaultProofPermit()`, matching the contract's 5-parameter signature `(proposalParent, proposalSignature, numExpiredPermits, numDelayedPermits, payoutRecipient)`.

#### Scenario: Acquiring a fault proof permit
- **WHEN** `acquire_fp_permit` calls `acquireFaultProofPermit`
- **THEN** the call SHALL include `0` as the `numDelayedPermits` argument between `numExpiredPermits` and `payoutRecipient`

### Requirement: releaseFaultProofPermit includes numDelayedPermits argument
The system SHALL pass `numDelayedPermits` as the fourth argument to `KailuaVerifier::releaseFaultProofPermit()`, matching the contract's 5-parameter signature `(proposalParent, proposalSignature, numExpiredPermits, numDelayedPermits, permitIndex)`.

#### Scenario: Releasing a fault proof permit
- **WHEN** `release_fp_permit` calls `releaseFaultProofPermit`
- **THEN** the call SHALL pass `permit_counts._1` (numDelayedPermits from countExpiredPermits) between `numExpiredPermits` and `permitIndex`

### Requirement: Return tuple indexing matches new ABI
The system SHALL use the correct return tuple indices for `countExpiredPermits`: `_0` = numExpiredPermits (u64), `_1` = numDelayedPermits (u64), `_2` = expiredCollateral (U256), `_3` = numActivePermits (u64).

#### Scenario: Total issued permits includes delayed permits
- **WHEN** computing `num_issued_permits` in `acquire_fp_permit`
- **THEN** the calculation SHALL be `num_expired_permits + num_delayed_permits + num_active_permits` (indices `_0 + _1 + _3`)

#### Scenario: Payout calculation uses correct indices
- **WHEN** computing payout in `release_fp_permit`
- **THEN** the calculation SHALL be `permit_counts._2 / U256::from(permit_counts._3)` (expiredCollateral / numActivePermits)

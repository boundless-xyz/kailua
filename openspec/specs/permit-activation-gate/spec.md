## ADDED Requirements

### Requirement: Cache permit timing constants at startup
The system SHALL load `PERMIT_DURATION` and `PERMIT_DELAY` from the KailuaVerifier contract during `SyncDeployment::load()` and store them as `u64` fields.

#### Scenario: Deployment loads permit timing constants
- **WHEN** `SyncDeployment::load()` initializes from the KailuaVerifier contract
- **THEN** `permit_duration` and `permit_delay` fields SHALL be populated from `KailuaVerifier::PERMIT_DURATION()` and `KailuaVerifier::PERMIT_DELAY()` respectively

### Requirement: Gate output fault proof submission on permit activation
The system SHALL check permit activation status before submitting an output fault proof (`proveOutputFault`). If the validator holds a permit with index > 0 and the permit is not yet active based on L1 block timestamp, the proof SHALL be re-buffered for later submission.

#### Scenario: Permit index 0 submits immediately
- **WHEN** the validator has a computed output fault proof AND holds a permit with index 0
- **THEN** the proof SHALL be submitted immediately without waiting for permit activation

#### Scenario: Permit index > 0 and permit not yet active
- **WHEN** the validator has a computed output fault proof AND holds a permit with index > 0 AND the latest L1 block timestamp is less than `permit_expiry - permit_duration + permit_delay`
- **THEN** the proof SHALL be re-buffered into `computed_proof_buffer` and submission SHALL be deferred

#### Scenario: Permit index > 0 and permit is active
- **WHEN** the validator has a computed output fault proof AND holds a permit with index > 0 AND the latest L1 block timestamp is greater than or equal to `permit_expiry - permit_duration + permit_delay`
- **THEN** the proof SHALL be submitted immediately

#### Scenario: No permit held
- **WHEN** the validator has a computed output fault proof AND holds no permit for the proposal
- **THEN** the proof SHALL be submitted immediately without any activation delay

### Requirement: Gate trail fault proof submission on permit activation
The system SHALL apply the same permit activation gate to trail fault proof submission (`proveTrailFault`), using identical logic to the output fault proof gate.

#### Scenario: Trail fault with permit index 0 submits immediately
- **WHEN** the validator has a trail fault proof ready AND holds a permit with index 0
- **THEN** the proof SHALL be submitted immediately

#### Scenario: Trail fault with permit index > 0 and permit not yet active
- **WHEN** the validator has a trail fault proof ready AND holds a permit with index > 0 AND the latest L1 block timestamp is less than `permit_expiry - permit_duration + permit_delay`
- **THEN** the proof SHALL be re-buffered into `trail_fault_buffer` with a retry time and submission SHALL be deferred

#### Scenario: Trail fault with permit index > 0 and permit is active
- **WHEN** the validator has a trail fault proof ready AND holds a permit with index > 0 AND the latest L1 block timestamp is greater than or equal to `permit_expiry - permit_duration + permit_delay`
- **THEN** the proof SHALL be submitted immediately

### Requirement: Use L1 block timestamp for permit timing
The system SHALL use the latest L1 block timestamp (not local system clock) when evaluating permit activation time, to match the contract's `block.timestamp` semantics.

#### Scenario: L1 timestamp used for activation check
- **WHEN** the system evaluates whether a permit is active
- **THEN** it SHALL fetch the latest L1 block via `validator_provider` and use its timestamp for the comparison

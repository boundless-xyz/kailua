## ADDED Requirements

### Requirement: Deterministic canonical hash of revm Cache structure

The system SHALL provide a function that computes a deterministic SHA256 hash of revm's `Cache` structure covering ALL fields: `accounts` (with `AccountInfo`, `storage`, and `AccountState` per account), `contracts` (code_hash to bytecode mapping), and `block_hashes` (block number to hash mapping). The hash SHALL be computed over a canonical byte encoding with sorted iteration order.

#### Scenario: identical Cache produces identical hash
- **WHEN** two `Cache` instances contain the same accounts, storage, contracts, and block_hashes
- **THEN** their hashes are identical regardless of insertion order

#### Scenario: any account change produces different hash
- **WHEN** a single account's nonce, balance, code_hash, storage slot, or account_state differs
- **THEN** the hash differs

#### Scenario: contract change produces different hash
- **WHEN** a contract bytecode entry is added, removed, or modified
- **THEN** the hash differs

#### Scenario: block_hash change produces different hash
- **WHEN** a block_hash entry is added, removed, or modified
- **THEN** the hash differs

#### Scenario: empty Cache has a well-defined hash
- **WHEN** a `Cache` with zero accounts, zero contracts, and zero block_hashes is hashed
- **THEN** the result is a deterministic, non-zero SHA256 value

### Requirement: Canonical byte encoding for Cache hashing

The canonical encoding for `Cache` hashing SHALL be:
1. Accounts: count (u32 BE), then for each account sorted by `Address`: address (20 bytes), nonce (u64 BE 8 bytes), balance (U256 BE 32 bytes), code_hash (32 bytes), account_state (1 byte), storage slot count (u32 BE), then for each slot sorted by `U256` key: slot (32 bytes), value (32 bytes).
2. Contracts: count (u32 BE), then for each contract sorted by code_hash: code_hash (32 bytes), bytecode length (u32 BE), bytecode raw bytes.
3. Block hashes: count (u32 BE), then for each entry sorted by block number (U256): block_number (U256 BE 32 bytes), hash (32 bytes).

Sections are concatenated in order: accounts || contracts || block_hashes.

#### Scenario: encoding is self-describing
- **WHEN** the canonical encoding is parsed
- **THEN** each section's length prefix allows unambiguous deserialization

#### Scenario: encoding sorts by natural key order
- **WHEN** accounts are inserted in random order
- **THEN** the encoding iterates them in ascending `Address` byte order

### Requirement: Deterministic canonical hash of EVM state accumulators

The system SHALL provide a function that computes a deterministic SHA256 hash of the EVM state accumulators. The accumulators SHALL include at minimum: `cumulative_gas_used` (u64), `da_footprint_used` (u64), `blob_gas_used` (u64), `logs_bloom` (Bloom, 256 bytes), and encoded receipts.

#### Scenario: identical EVM state produces identical hash
- **WHEN** two EVM state accumulator sets have the same values
- **THEN** their hashes are identical

#### Scenario: initial EVM state has well-defined hash
- **WHEN** the EVM state is initialized with all-zero accumulators and empty receipts
- **THEN** the hash is a deterministic, non-zero SHA256 value

#### Scenario: receipt ordering is preserved
- **WHEN** receipts are accumulated during chunk execution
- **THEN** the hash includes receipts in transaction execution order

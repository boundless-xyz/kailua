## ADDED Requirements

### Requirement: Canonical state normalization across revm representations

The system SHALL provide a canonical normalization layer that produces identical byte encodings from both revm's `Cache` (from `CacheDB`, using `DbAccount`/`AccountState`) and a state-backed effective flat view derived from `CacheState` plus `State.block_hashes` (from `State`, using `CacheAccount`/`PlainAccount`/`AccountStatus`). The normalization SHALL map `AccountStatus` values to their `AccountState` equivalents for HASHING ONLY (`Loaded`→`None`, `Changed`→`Touched`, `Destroyed`→`StorageCleared`, `InMemoryChange`→`Touched`, etc.) and extract account data (nonce, balance, code_hash, storage) uniformly from both `DbAccount` and `PlainAccount`.

When execution starts from a preloaded `CacheDB` base cache (for example chunk proving with `State<CacheDB<PanicDB>>`), the canonical post-state hash SHALL be computed from the effective flat state obtained by overlaying the projected final `CacheState` and final `State.block_hashes` onto the initial `Cache`, rather than from bare `CacheState` alone. When there is no hidden preloaded `Cache` layer beneath `State` (for example direct trie-backed aggregation state), the live `CacheState` plus `State.block_hashes` is itself the effective flat view.

#### Scenario: Cache and CacheState with same logical state produce identical hashes
- **WHEN** a `Cache` and a `CacheState` represent the same accounts, storage, contracts, and block_hashes
- **THEN** the canonical hash of both representations is identical

#### Scenario: state-backed post-state hashing preserves untouched preloaded entries
- **WHEN** execution begins from an initial `Cache`, some witness entries are never touched, and the final live state is represented by `CacheState` plus `State.block_hashes`
- **THEN** the canonical post-state hash is computed from the effective flat state formed by overlaying the final live projection onto the initial `Cache`, so untouched preloaded entries still contribute to the hash

#### Scenario: AccountStatus to AccountState mapping is deterministic
- **WHEN** an account has `AccountStatus::Changed` in `CacheState`
- **THEN** it normalizes to `AccountState::Touched` for hashing

### Requirement: Deterministic canonical hash of normalized state

The system SHALL provide a function that computes a deterministic SHA256 hash from the normalized state representation, covering the execution-relevant flat state fields: `accounts` (with `AccountInfo`, `storage`, and normalized account state per account), `contracts` (code_hash to bytecode mapping), and `block_hashes` (block number to hash mapping). `Cache.logs` is intentionally EXCLUDED because logs are already committed by the EVM accumulator hash through ordered receipts and logs bloom. The hash SHALL be computed over a canonical byte encoding with sorted iteration order.

#### Scenario: identical state produces identical hash
- **WHEN** two state representations contain the same accounts, storage, contracts, and block_hashes
- **THEN** their hashes are identical regardless of insertion order or source representation

#### Scenario: any account change produces different hash
- **WHEN** a single account's nonce, balance, code_hash, storage slot, or account_state differs
- **THEN** the hash differs

#### Scenario: contract change produces different hash
- **WHEN** a contract bytecode entry is added, removed, or modified
- **THEN** the hash differs

#### Scenario: block_hash change produces different hash
- **WHEN** a block_hash entry is added, removed, or modified
- **THEN** the hash differs

#### Scenario: log-only change does not affect state hash
- **WHEN** only `Cache.logs` changes while accounts, contracts, and block_hashes remain identical
- **THEN** the canonical state hash is unchanged because logs are committed separately via the EVM accumulator

#### Scenario: empty state has a well-defined hash
- **WHEN** a state with zero accounts, zero contracts, and zero block_hashes is hashed
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

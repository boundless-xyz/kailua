## ADDED Requirements

### Requirement: Dependencies installed as git submodules
The project SHALL manage OP Stack and RISC Zero Ethereum dependencies as git submodules under `lib/`, installed via `forge install` or equivalent.

#### Scenario: OP Stack v5.0.0 submodule present
- **WHEN** checking `lib/` directory contents
- **THEN** `lib/optimism` SHALL exist as a git submodule pinned to the `op-contracts/v5.0.0` tag

#### Scenario: RISC Zero v3.0.1 submodule present
- **WHEN** checking `lib/` directory contents
- **THEN** `lib/risc0-ethereum` SHALL exist as a git submodule pinned to the `v3.0.1` tag

#### Scenario: forge-std unchanged
- **WHEN** checking existing dependencies
- **THEN** `lib/forge-std` SHALL remain at its current version (v1.9.6) and be unmodified

### Requirement: Vendored flat files removed
The project SHALL NOT contain vendored flat contract files after migration.

#### Scenario: Vendor directory cleaned up
- **WHEN** checking `src/vendor/` directory
- **THEN** `FlatOPImportV1.4.0.sol` and `FlatR0ImportV2.0.2.sol` SHALL NOT exist
- **AND** the `src/vendor/` directory SHALL be removed if empty

### Requirement: Import remappings configured in foundry.toml
The project SHALL configure Foundry remappings to provide clean import paths for all dependencies.

#### Scenario: OP Stack source contracts remapped
- **WHEN** importing OP Stack source contracts (e.g., `DisputeGameFactory`, `OptimismPortal2`)
- **THEN** imports SHALL use a prefix remapping (e.g., `@opstack/`) that resolves to `lib/optimism/packages/contracts-bedrock/src/`

#### Scenario: OP Stack interfaces remapped
- **WHEN** importing OP Stack interfaces (e.g., `ISemver`, `IDisputeGame`, `IDisputeGameFactory`)
- **THEN** imports SHALL use a prefix remapping (e.g., `@opstack-interfaces/`) that resolves to `lib/optimism/packages/contracts-bedrock/interfaces/`

#### Scenario: RISC Zero contracts remapped
- **WHEN** importing RISC Zero contracts (e.g., `IRiscZeroVerifier`, `RiscZeroGroth16Verifier`)
- **THEN** imports SHALL use a prefix remapping (e.g., `@risc0/`) that resolves to `lib/risc0-ethereum/contracts/src/`

#### Scenario: Transitive dependencies remapped
- **WHEN** upstream dependencies require OpenZeppelin or other transitive dependencies
- **THEN** remappings SHALL resolve them to the copies bundled within the upstream submodules (e.g., `@openzeppelin/contracts/` → `lib/optimism/.../lib/openzeppelin-contracts/contracts/`)

### Requirement: All import paths updated to use remappings
All Solidity files in `src/`, `test/`, and `scripts/` SHALL use the configured remapping prefixes instead of vendor file imports.

#### Scenario: Source contract imports updated
- **WHEN** compiling any file in `src/` (KailuaLib.sol, KailuaTournament.sol, KailuaVerifier.sol, KailuaTreasury.sol, KailuaGame.sol)
- **THEN** no file SHALL contain `import "./vendor/FlatOPImportV1.4.0.sol"` or `import "./vendor/FlatR0ImportV2.0.2.sol"`
- **AND** each file SHALL import only the specific symbols it uses via named imports from remapped paths

#### Scenario: Test file imports updated
- **WHEN** compiling any file in `test/`
- **THEN** no file SHALL contain vendor file imports
- **AND** imports SHALL use the same remapping prefixes as source contracts

#### Scenario: Script file imports updated
- **WHEN** compiling any file in `scripts/`
- **THEN** no file SHALL contain vendor file imports
- **AND** imports SHALL use the same remapping prefixes as source contracts

### Requirement: Contracts adapted to OP Stack v5 API
Kailua contracts SHALL compile and function correctly against OP Stack v5.0.0 APIs.

#### Scenario: OP Stack type compatibility
- **WHEN** using OP Stack types (`Claim`, `Hash`, `GameType`, `Timestamp`, `Duration`, `GameStatus`)
- **THEN** types SHALL be imported from their v5 locations (e.g., `@opstack/dispute/lib/Types.sol` or `@opstack/dispute/lib/LibUDT.sol`)
- **AND** any renamed types SHALL use the v5 names

#### Scenario: OP Stack contract interface compatibility
- **WHEN** Kailua contracts reference `OptimismPortal2`, `DisputeGameFactory`, `SystemConfig`, or `SuperchainConfig`
- **THEN** all method calls, constructor parameters, and type casts SHALL be compatible with the v5 contract interfaces
- **AND** any methods that changed signatures SHALL be updated accordingly

#### Scenario: OP Stack interface path compatibility
- **WHEN** Kailua contracts implement or reference `ISemver`, `IDisputeGame`, `IDisputeGameFactory`, or `IInitializable`
- **THEN** interfaces SHALL be imported from the v5 `interfaces/` directory via the `@opstack-interfaces/` remapping

### Requirement: Contracts adapted to RISC Zero v3 API
Kailua contracts SHALL compile and function correctly against RISC Zero Ethereum v3.0.1 APIs.

#### Scenario: RISC Zero verifier compatibility
- **WHEN** using `IRiscZeroVerifier`, `RiscZeroVerifierRouter`, `RiscZeroGroth16Verifier`, or `RiscZeroMockVerifier`
- **THEN** contracts SHALL be imported from their v3.0.1 locations via the `@risc0/` remapping
- **AND** all method calls SHALL be compatible with the v3 interfaces

#### Scenario: RISC Zero library compatibility
- **WHEN** using `ReceiptClaimLib` in test contracts
- **THEN** the library SHALL be imported from `@risc0/IRiscZeroVerifier.sol`
- **AND** all library method calls (`digest()`, `ok()`) SHALL remain compatible

### Requirement: Project compiles successfully
The project SHALL compile without errors after all changes.

#### Scenario: forge build succeeds
- **WHEN** running `forge build` in the foundry project directory
- **THEN** compilation SHALL succeed with zero errors

#### Scenario: Solidity version compatible
- **WHEN** compiling with the configured Solidity version
- **THEN** all source files (including dependencies) SHALL be compatible with the compiler version specified in `foundry.toml`

### Requirement: All tests pass
All existing tests SHALL continue to pass after the migration.

#### Scenario: forge test succeeds
- **WHEN** running `forge test` in the foundry project directory
- **THEN** all test cases SHALL pass

### Requirement: CI workflow updated
The CI workflow SHALL correctly handle git submodule dependencies.

#### Scenario: GitHub Actions checkout with submodules
- **WHEN** the CI workflow checks out the repository
- **THEN** the checkout step SHALL include recursive submodule initialization (e.g., `submodules: recursive`)

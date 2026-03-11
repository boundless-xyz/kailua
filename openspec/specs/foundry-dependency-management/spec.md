## ADDED Requirements

### Requirement: Dependencies installed as git submodules
The project SHALL manage OP Stack and RISC Zero Ethereum dependencies as git submodules under `lib/`, installed via `git submodule add` or equivalent.

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
- **AND** the `src/vendor/` directory SHALL be removed

### Requirement: Import remappings configured in foundry.toml
The project SHALL configure Foundry remappings to provide clean import paths for all dependencies.

#### Scenario: OP Stack source contracts remapped
- **WHEN** importing OP Stack source contracts (types, errors, libraries)
- **THEN** imports SHALL use the `@optimism/src/` prefix that resolves to `lib/optimism/packages/contracts-bedrock/src/`

#### Scenario: OP Stack interfaces remapped
- **WHEN** importing OP Stack interfaces (e.g., `ISemver`, `IDisputeGame`, `IDisputeGameFactory`, `IOptimismPortal2`)
- **THEN** imports SHALL use the `@optimism/interfaces/` prefix that resolves to `lib/optimism/packages/contracts-bedrock/interfaces/`

#### Scenario: RISC Zero contracts remapped
- **WHEN** importing RISC Zero contracts (e.g., `IRiscZeroVerifier`, `RiscZeroGroth16Verifier`)
- **THEN** imports SHALL use the `@risc0/` prefix that resolves to `lib/risc0-ethereum/contracts/src/`

#### Scenario: Transitive dependencies remapped
- **WHEN** upstream dependencies require OpenZeppelin, solady, or other transitive dependencies
- **THEN** remappings SHALL resolve them to the copies bundled within the upstream submodules (e.g., `@openzeppelin/contracts/` → `lib/optimism/.../lib/openzeppelin-contracts/contracts/`)

### Requirement: All import paths updated to use remappings
All Solidity files in `src/`, `test/`, and `scripts/` SHALL use the configured remapping prefixes instead of vendor file imports.

#### Scenario: Source contract imports updated
- **WHEN** compiling any file in `src/` (KailuaLib.sol, KailuaTournament.sol, KailuaVerifier.sol, KailuaTreasury.sol, KailuaGame.sol)
- **THEN** no file SHALL contain `import "./vendor/FlatOPImportV1.4.0.sol"` or `import "./vendor/FlatR0ImportV2.0.2.sol"`
- **AND** each file SHALL import only the specific symbols it uses via named imports from `@optimism/` or `@risc0/` remapped paths

#### Scenario: Test file imports updated
- **WHEN** compiling any file in `test/`
- **THEN** no file SHALL contain vendor file imports
- **AND** imports SHALL use the same `@optimism/` and `@risc0/` remapping prefixes as source contracts

#### Scenario: Script file imports updated
- **WHEN** compiling any file in `scripts/`
- **THEN** no file SHALL contain vendor file imports
- **AND** imports SHALL use the same `@optimism/` and `@risc0/` remapping prefixes as source contracts

### Requirement: Contracts adapted to OP Stack v5 API
Kailua contracts SHALL compile and function correctly against OP Stack v5.0.0 APIs.

#### Scenario: OP Stack type compatibility
- **WHEN** using OP Stack types (`Claim`, `Hash`, `GameType`, `Timestamp`, `Duration`)
- **THEN** types SHALL be imported from `@optimism/src/dispute/lib/Types.sol`
- **AND** `GameStatus` SHALL be imported from `@optimism/interfaces/dispute/IDisputeGame.sol` to avoid duplicate type definitions

#### Scenario: OP Stack contract interface compatibility
- **WHEN** Kailua contracts reference `OptimismPortal2`, `DisputeGameFactory`, `SystemConfig`, or `SuperchainConfig`
- **THEN** references SHALL use interface types (`IOptimismPortal2`, `IDisputeGameFactory`) to avoid pragma version conflicts (v5 concrete contracts require 0.8.15)
- **AND** tests SHALL use mock contracts instead of concrete OP Stack implementations

#### Scenario: OP Stack interface path compatibility
- **WHEN** Kailua contracts implement or reference `ISemver`, `IDisputeGame`, `IDisputeGameFactory`, or `IInitializable`
- **THEN** interfaces SHALL be imported from `@optimism/interfaces/`

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
The project SHALL compile without errors after all changes, both via `forge build` and `cargo build`.

#### Scenario: forge build succeeds
- **WHEN** running `forge build` in the foundry project directory
- **THEN** compilation SHALL succeed with zero errors

#### Scenario: cargo build succeeds
- **WHEN** running `cargo build -p kailua-contracts` (without `--features skip-solc`)
- **THEN** the build script SHALL compile all Solidity contracts using `foundry_compilers` with the configured remappings
- **AND** compilation SHALL succeed with zero errors

#### Scenario: Solidity version compatible
- **WHEN** compiling with the configured Solidity version (0.8.24)
- **THEN** all source files SHALL be compatible
- **AND** the solar linter MAY be disabled (`lint_on_build = false`) if it cannot resolve remapping paths

### Requirement: All tests pass
All existing tests SHALL continue to pass after the migration.

#### Scenario: forge test succeeds
- **WHEN** running `forge test` in the foundry project directory
- **THEN** all test cases SHALL pass

### Requirement: build.rs reads remappings from remappings.txt
The Rust build script (`crates/contracts/build.rs`) SHALL read import remappings from `foundry/remappings.txt` and pass them to the `foundry_compilers` project configuration.

#### Scenario: Remappings loaded at build time
- **WHEN** `cargo build -p kailua-contracts` runs the build script
- **THEN** the build script SHALL read `foundry/remappings.txt`
- **AND** each non-empty line SHALL be parsed as a `Remapping` using `Remapping::from_str()`
- **AND** the parsed remappings SHALL be set on the `ProjectPathsConfig` before compilation

#### Scenario: Remapped imports resolve correctly
- **WHEN** a Solidity source file contains `import {X} from "@optimism/src/..."` or similar `@`-prefixed imports
- **THEN** the compiler SHALL resolve the import via the loaded remappings to the correct file in the `lib/` submodule tree

#### Scenario: Missing remappings.txt
- **WHEN** `foundry/remappings.txt` does not exist
- **THEN** the build script SHALL proceed with no remappings (preserving backward compatibility with the `skip-solc` feature or empty projects)

### Requirement: build.rs triggers rebuild on dependency changes
The build script SHALL declare `cargo:rerun-if-changed` directives for all inputs that affect Solidity compilation.

#### Scenario: Remappings file change triggers rebuild
- **WHEN** `foundry/remappings.txt` is modified
- **THEN** the next `cargo build` SHALL re-run the build script

#### Scenario: Foundry config change triggers rebuild
- **WHEN** `foundry/foundry.toml` is modified
- **THEN** the next `cargo build` SHALL re-run the build script

#### Scenario: Library directory change triggers rebuild
- **WHEN** any file under `foundry/lib/` changes (e.g., submodule update)
- **THEN** the next `cargo build` SHALL re-run the build script

### Requirement: Rust ABI bindings reference the current Foundry artifact layout
The Rust contract bindings in `crates/contracts/src/lib.rs` SHALL reference artifact JSON files that exist in the package-based Foundry layout.

#### Scenario: Current artifact paths used for Rust bindings
- **WHEN** `cargo build -p kailua-contracts` expands the `alloy::sol!` macros in `crates/contracts/src/lib.rs`
- **THEN** each referenced JSON path SHALL exist under `foundry/out/` or an intentionally selected checked-in ABI snapshot path
- **AND** the build SHALL NOT depend on removed `Flat*.sol` artifact files

#### Scenario: Deployable RISC Zero artifacts are generated during project compilation
- **WHEN** the Rust crate needs bindings for `RiscZeroVerifierRouter` or `RiscZeroGroth16Verifier`
- **THEN** the Foundry project SHALL include source imports that cause those contracts to be emitted under `foundry/out/`
- **AND** the resulting artifacts SHALL contain the bytecode required by deployable Rust bindings

#### Scenario: RISC Zero transitive OpenZeppelin imports resolve
- **WHEN** `foundry_compilers` compiles the imported `RiscZeroGroth16Verifier` source during `cargo build`
- **THEN** the compiler SHALL resolve `openzeppelin/contracts/...` imports via an explicit remapping
- **AND** compilation SHALL succeed without missing-file import errors

### Requirement: CI workflow updated
The CI workflow SHALL correctly handle git submodule dependencies.

#### Scenario: GitHub Actions checkout with submodules
- **WHEN** the CI workflow checks out the repository
- **THEN** the checkout step SHALL include recursive submodule initialization (e.g., `submodules: recursive`)

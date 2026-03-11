## MODIFIED Requirements

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

## ADDED Requirements

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

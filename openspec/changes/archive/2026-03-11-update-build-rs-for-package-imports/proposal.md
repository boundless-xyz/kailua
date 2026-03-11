## Why

The `crates/contracts/build.rs` uses `foundry_compilers::ProjectPathsConfig::builder().build_with_root("foundry")` to compile Solidity contracts, but this API does not read remappings from `foundry.toml`. After migrating from vendored flat Solidity files to git submodule dependencies with namespace-based remappings (`@optimism/`, `@risc0/`, `@solady/`, etc.), the Rust build fails because `solc` cannot resolve any of the new import paths.

Once remappings are loaded correctly, `cargo build -p kailua-contracts` still fails later in Rust compilation because `crates/contracts/src/lib.rs` still points `alloy::sol!` bindings at removed `Flat*.sol` artifact paths. The new package-based Foundry layout emits different artifact paths, and deployable dependency contracts like `RiscZeroVerifierRouter` and `RiscZeroGroth16Verifier` are not generated unless the Foundry project explicitly imports them.

## What Changes

- Update `build.rs` to read remappings from `foundry/remappings.txt` and pass them to the `foundry_compilers` project configuration
- Pin the build script to the `solc_version` configured in `foundry.toml` when that compiler is installed via `svm`
- Ensure `cargo:rerun-if-changed` watches `remappings.txt` and `foundry.toml` for rebuild triggers
- Ensure `cargo:rerun-if-changed` watches `lib/` submodule directories so changes to dependencies trigger recompilation
- Update `crates/contracts/src/lib.rs` to use current Foundry artifact paths or stable ABI JSONs for Rust bindings
- Add a small Foundry shim source to force generation of deployable RISC Zero artifact JSONs needed by Rust bindings
- Add an explicit `openzeppelin/contracts/` remapping for the RISC Zero Groth16 dependency tree so `foundry_compilers` can resolve transitive imports consistently

## Capabilities

### New Capabilities

_None_ - this is a fix to an existing build mechanism, not a new capability.

### Modified Capabilities

- `foundry-dependency-management`: The build system requirement changes to support remapping-based imports in the Rust compilation pipeline (not just `forge build`).

## Impact

- **Code**: `crates/contracts/build.rs`, `crates/contracts/src/lib.rs`, `crates/contracts/foundry/foundry.toml`, `crates/contracts/foundry/remappings.txt`, `crates/contracts/foundry/src/RustBindingsImports.sol`
- **Dependencies**: No new Rust dependencies needed (`foundry_compilers` already exposes `Remapping` types)
- **Build**: Unblocks `cargo build -p kailua-contracts` both at the Solidity compilation phase and the Rust ABI binding phase
- **CI**: No changes needed (CI already uses `submodules: recursive`)

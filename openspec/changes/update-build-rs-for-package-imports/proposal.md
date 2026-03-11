## Why

The `crates/contracts/build.rs` uses `foundry_compilers::ProjectPathsConfig::builder().build_with_root("foundry")` to compile Solidity contracts, but this API does not read remappings from `foundry.toml`. After migrating from vendored flat Solidity files to git submodule dependencies with namespace-based remappings (`@optimism/`, `@risc0/`, `@solady/`, etc.), the Rust build fails because `solc` cannot resolve any of the new import paths.

## What Changes

- Update `build.rs` to read remappings from `foundry/remappings.txt` and pass them to the `foundry_compilers` project configuration
- Ensure `cargo:rerun-if-changed` watches `remappings.txt` and `foundry.toml` for rebuild triggers
- Ensure `cargo:rerun-if-changed` watches `lib/` submodule directories so changes to dependencies trigger recompilation

## Capabilities

### New Capabilities

_None_ - this is a fix to an existing build mechanism, not a new capability.

### Modified Capabilities

- `foundry-dependency-management`: The build system requirement changes to support remapping-based imports in the Rust compilation pipeline (not just `forge build`).

## Impact

- **Code**: `crates/contracts/build.rs` only
- **Dependencies**: No new Rust dependencies needed (`foundry_compilers` already exposes `Remapping` types)
- **Build**: Unblocks `cargo build -p kailua-contracts` which currently fails with `ParserError: Source "@optimism/..." not found`
- **CI**: No changes needed (CI already uses `submodules: recursive`)

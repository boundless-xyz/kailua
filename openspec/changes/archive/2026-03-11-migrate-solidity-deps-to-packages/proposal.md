## Why

The Solidity project in `crates/contracts/foundry/` currently vendors its OP Stack (v1.4.0) and RISC Zero (v2.0.2) dependencies as flattened single-file blobs (`FlatOPImportV1.4.0.sol` at ~7,500 lines, `FlatR0ImportV2.0.2.sol` at ~2,300 lines). This makes upgrades painful, obscures what's actually imported, bloats the source tree, and prevents proper dependency management. We need to upgrade to OP Stack contracts v5.0.0 and RISC Zero Ethereum contracts v3.0.1, and this is the right time to switch from vendoring to proper Foundry package imports.

## What Changes

- **Remove vendored contract files**: Delete `src/vendor/FlatOPImportV1.4.0.sol` and `src/vendor/FlatR0ImportV2.0.2.sol`
- **Add OP Stack v5 as a Foundry dependency**: Install `ethereum-optimism/optimism` at tag `op-contracts/v5.0.0` via `forge install` or git submodule
- **Add RISC Zero Ethereum v3.0.1 as a Foundry dependency**: Install `risc0/risc0-ethereum` at tag `v3.0.1` via `forge install` or git submodule
- **BREAKING**: Update all import paths across `src/`, `test/`, and `scripts/` from flat vendor imports to proper package imports pointing to individual contract/library files
- **Update `foundry.toml`**: Add remappings or create `remappings.txt` to map package import paths
- **BREAKING**: Adapt Kailua contracts to any breaking API changes between OP Stack v1.4.0 → v5.0.0 and RISC Zero v2.0.2 → v3.0.1

## Capabilities

### New Capabilities
- `foundry-dependency-management`: Proper Foundry/forge dependency management using git submodules and remappings instead of vendored flat files

### Modified Capabilities
_(none - no existing spec-level behavior changes, only implementation/dependency changes)_

## Impact

- **Code**: All 5 Kailua source contracts, 10 test files, and 1 deployment script will need import path updates. Contracts may need API adaptation for breaking changes in upstream dependencies.
- **Dependencies**: `lib/` directory gains two new git submodules (optimism, risc0-ethereum). Existing `lib/forge-std` is unaffected.
- **Build**: `foundry.toml` or `remappings.txt` needs remapping configuration. Solidity compiler version may need adjustment if upstream requires different version.
- **CI**: `.github/workflows/test.yml` may need `--recurse-submodules` for checkout.

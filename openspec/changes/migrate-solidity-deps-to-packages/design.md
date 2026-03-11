## Context

The Kailua Solidity project at `crates/contracts/foundry/` currently vendors two large flattened files:
- `src/vendor/FlatOPImportV1.4.0.sol` (~7,500 lines) - OP Stack contracts from v1.4.0
- `src/vendor/FlatR0ImportV2.0.2.sol` (~2,300 lines) - RISC Zero Ethereum contracts from v2.0.2

All 5 source contracts, 10 test files, and 1 deployment script import from these flat files. The project uses Solidity 0.8.24 with Foundry (forge-std v1.9.6 is the only `lib/` dependency).

We are upgrading to OP Stack contracts v5.0.0 and RISC Zero Ethereum contracts v3.0.1, switching from vendored flat files to proper Foundry git submodule dependencies.

### Key Upstream Changes

**OP Stack v1.4.0 → v5.0.0:**
- Interfaces moved to top-level `interfaces/` directory (from `src/`)
- `OutputRoot` struct renamed to `Proposal` with `l2BlockNumber` → `l2SequenceNumber`
- New `BondDistributionMode` enum
- `DisputeGameFactory` version 1.0.0 → 1.3.0 (new inheritance, storage, functions)
- `OptimismPortal2` version 3.10.0 → 5.1.1 (AnchorStateRegistry indirection, ETHLockbox)
- `SuperchainConfig` version 1.1.0 → 2.4.0 (per-identifier pause model)
- `GameTypes` expanded from 4 to 14 types (includes `KAILUA(1337)`)
- `IDisputeGame` added `l2SequenceNumber()`, `wasRespectedGameTypeWhenCreated()`
- Pragma: `0.8.15` (exact) for core contracts, `^0.8.0` or `^0.8.15` for libraries

**RISC Zero v2.0.2 → v3.0.1:**
- No breaking Solidity API changes
- `ControlID.CONTROL_ROOT` constant changed (new circuit identity)
- `RiscZeroGroth16Verifier.VERSION` → `"3.0.0"`
- Solidity version: `0.8.26` in foundry.toml, `^0.8.9` pragma
- `via_ir = true` required in foundry config

### Symbols Used by Kailua

**From OP Stack (18 symbols):**
- Types: `Claim`, `Hash`, `GameType`, `Timestamp`, `Duration`
- Enums: `GameStatus`
- Interfaces: `IDisputeGame`, `IDisputeGameFactory`, `IInitializable`, `ISemver`
- Contracts: `Clone`, `OptimismPortal2`, `DisputeGameFactory`, `SystemConfig`, `SuperchainConfig`, `OwnableUpgradeable`
- Libraries: `LibClone`

**From RISC Zero (5 symbols):**
- Interfaces: `IRiscZeroVerifier`
- Contracts: `RiscZeroMockVerifier`, `RiscZeroVerifierRouter`, `RiscZeroGroth16Verifier`
- Libraries: `ReceiptClaimLib`

## Goals / Non-Goals

**Goals:**
- Replace vendored flat files with proper Foundry git submodule dependencies
- Upgrade OP Stack contracts to v5.0.0
- Upgrade RISC Zero Ethereum contracts to v3.0.1
- Update all import paths to use package-style imports with remappings
- Adapt Kailua contracts to any breaking API changes in upstream dependencies
- Maintain all existing contract functionality and test coverage

**Non-Goals:**
- Refactoring Kailua contract logic beyond what's needed for API compatibility
- Upgrading forge-std or other unrelated dependencies
- Adding new features or capabilities to Kailua contracts
- Changing the deployment workflow beyond necessary CI adjustments
- Upgrading OpenZeppelin independently (it comes transitively through upstream deps)

## Decisions

### 1. Dependency Installation Method: `forge install` with git submodules

**Decision:** Use `forge install` to add both dependencies as git submodules under `lib/`.

**Rationale:** This is the standard Foundry approach, consistent with the existing `lib/forge-std` setup. It provides version pinning, reproducible builds, and easy updates via `forge update`.

**Alternative considered:** Soldeer package manager — rejected because it would introduce a new tool dependency and the project already uses the git submodule pattern.

### 2. Import Remapping Strategy

**Decision:** Add remappings to `foundry.toml` (not a separate `remappings.txt`).

The OP Stack repo has a complex internal structure. We will use remappings to create clean import paths:

```toml
remappings = [
  # OP Stack v5
  "@opstack/=lib/optimism/packages/contracts-bedrock/src/",
  "@opstack-interfaces/=lib/optimism/packages/contracts-bedrock/interfaces/",
  # OP Stack's own transitive deps
  "@openzeppelin/contracts/=lib/optimism/packages/contracts-bedrock/lib/openzeppelin-contracts/contracts/",
  "@openzeppelin/contracts-upgradeable/=lib/optimism/packages/contracts-bedrock/lib/openzeppelin-contracts-upgradeable/contracts/",
  # RISC Zero v3
  "@risc0/=lib/risc0-ethereum/contracts/src/",
  # forge-std (keep existing)
  "forge-std/=lib/forge-std/src/",
]
```

**Rationale:** Consolidating remappings in `foundry.toml` keeps configuration in one place. The `@opstack/` and `@risc0/` prefixes provide clean, memorable import paths and avoid conflicts.

**Alternative considered:** Using the raw submodule paths directly (e.g., `lib/optimism/packages/contracts-bedrock/src/...`) — rejected because it's verbose and fragile to directory structure changes.

### 3. Solidity Compiler Version

**Decision:** Keep `solc_version = "0.8.24"` in foundry.toml.

**Rationale:** OP Stack v5 core contracts use `pragma solidity 0.8.15` (exact), and RISC Zero uses `^0.8.9`. Solidity 0.8.24 satisfies all pragmas. RISC Zero's foundry.toml specifies 0.8.26, but `^0.8.9` pragma allows 0.8.24.

**Risk:** May need to bump to 0.8.26 if there are compilation issues. This is low risk since both are patch-level differences.

### 4. Handling OP Stack v5 Breaking Changes

**Decision:** Adapt Kailua contracts to the v5 API:

- **Interface imports:** Update from `src/` paths to `interfaces/` paths via `@opstack-interfaces/` remapping
- **`OutputRoot` → `Proposal`:** If Kailua uses this struct, update references. (Based on analysis, Kailua doesn't directly use `OutputRoot`, so this may be a no-op)
- **`DisputeGameFactory` changes:** The Kailua contracts primarily use it as a type for storage/parameters — verify the interface hasn't changed for the methods Kailua calls
- **`OptimismPortal2` changes:** Same as above — verify the methods Kailua calls still exist with compatible signatures
- **`SuperchainConfig` changes:** Kailua only instantiates it in tests — adapt constructor/initialize calls

### 5. Handling `via_ir` Requirement

**Decision:** Add `via_ir = true` to `foundry.toml` if required by RISC Zero contracts.

**Rationale:** RISC Zero's foundry.toml requires `via_ir = true`. If the Groth16Verifier or other contracts fail to compile without it, we must enable it. This may increase compilation time but ensures compatibility.

### 6. Transitive Dependency Management

**Decision:** Use the OP Stack's vendored OpenZeppelin contracts via remappings pointing into the optimism submodule.

**Rationale:** OP Stack v5 depends on OpenZeppelin v4 (`@openzeppelin/contracts/`) and v5 (`@openzeppelin/contracts-v5/`). Rather than installing OpenZeppelin separately, we reuse the versions bundled with the optimism submodule. RISC Zero also depends on OpenZeppelin — if there are conflicts, we may need a separate installation.

**Risk:** Version conflicts between OP Stack and RISC Zero OpenZeppelin requirements. Mitigation: RISC Zero uses `openzeppelin/=../lib/openzeppelin-contracts/` which can be remapped to use the OP Stack's copy if compatible, or a separate installation if not.

### 7. `LibClone` (solady) Dependency

**Decision:** The `LibClone` library used in tests comes from solady (via OP Stack). Ensure the remapping includes solady or add a direct dependency.

**Rationale:** OP Stack v5 includes `@solady/=lib/solady/src/` in its remappings. We can either remap to the OP Stack's bundled solady or install it directly.

## Risks / Trade-offs

- **[Large repository size]** → The optimism monorepo is large (~1GB+). Mitigation: Use `--no-commit --no-tag` with forge install, and consider shallow cloning or sparse checkout if size is problematic. CI may need `--recurse-submodules` and possibly `--depth 1`.

- **[Compilation time increase]** → `via_ir = true` significantly increases compile time. Mitigation: Only enable if required. Consider using profiles (e.g., `via_ir = true` only for production builds, not tests).

- **[Transitive dependency conflicts]** → OP Stack and RISC Zero may require different OpenZeppelin versions. Mitigation: Use targeted remappings that resolve each dependency's OpenZeppelin to its own bundled copy.

- **[OP Stack API changes may require deeper Kailua refactoring]** → If `OptimismPortal2` or `DisputeGameFactory` methods that Kailua calls have changed signatures. Mitigation: Carefully audit each call site against v5 interfaces. The Kailua contracts primarily use these as types for storage/parameters rather than calling complex methods directly.

- **[CI checkout time]** → Recursive submodule checkout of the optimism repo will slow CI. Mitigation: Use `--depth 1` for submodule cloning in CI.

## Open Questions

1. **Should we use `forge install` or manually add git submodules?** `forge install` is cleaner but may not support all options (e.g., sparse checkout). Manual git submodule gives more control.

2. **Is the full optimism monorepo needed, or can we use a contracts-only package?** The optimism repo packages contracts under `packages/contracts-bedrock/` — there's no standalone contracts package published for Foundry.

3. **Do we need `Safe` contracts?** The current vendor file includes Safe (Gnosis Safe). If Kailua doesn't directly use Safe, we may not need this dependency. If it's required transitively by OP Stack contracts we use, it comes bundled.

4. **Should we keep the vendor directory for any contracts?** If some OP Stack contracts we need aren't exported cleanly or have compilation issues, we may need to keep selective vendoring. Goal is to eliminate it entirely.

## 1. Install Dependencies

- [x] 1.1 Install OP Stack v5.0.0 as git submodule under `lib/optimism` (pinned to `op-contracts/v5.0.0`)
- [x] 1.2 Install RISC Zero Ethereum v3.0.1 as git submodule under `lib/risc0-ethereum` (pinned to `v3.0.1`)
- [x] 1.3 Initialize transitive submodules required by OP Stack and RISC Zero (OpenZeppelin, solady, etc.)

## 2. Configure Foundry

- [x] 2.1 Add remappings to `foundry.toml` using `@optimism/src/` and `@optimism/interfaces/` prefixes for OP Stack, `@risc0/` for RISC Zero, plus transitive dependency remappings
- [x] 2.2 Disabled solar linter (`lint_on_build = false`) — solar cannot resolve `@optimism/src/` remapping correctly
- [x] 2.3 Verified Solidity version compatibility — kept 0.8.24, use interfaces only (no concrete v5 contracts with pragma 0.8.15)
- [x] 2.4 `.gitignore` — not needed for submodules; `optimism/` in root gitignore required `-f` flag for `git submodule add`

## 3. Update Source Contract Imports

- [x] 3.1 Update `KailuaLib.sol`: Replace vendor imports with `@optimism/src/` named imports
- [x] 3.2 Update `KailuaTournament.sol`: Replace vendor imports, use `@optimism/interfaces/` for `IDisputeGame`, `IDisputeGameFactory`, `IOptimismPortal2`; import `GameStatus` from interface
- [x] 3.3 Update `KailuaVerifier.sol`: Replace vendor imports with `@optimism/interfaces/` for `ISemver`, `@optimism/src/` for `Duration`, `@risc0/` for `IRiscZeroVerifier`
- [x] 3.4 Update `KailuaTreasury.sol`: Replace vendor imports with `@optimism/` prefixed imports, add `IInitializable`; import `GameStatus` from `IDisputeGame` interface
- [x] 3.5 Update `KailuaGame.sol`: Replace vendor imports with `@optimism/` prefixed imports, add `IInitializable`; import `GameStatus` from `IDisputeGame` interface

## 4. Adapt Source Contracts to v5 API Changes

- [x] 4.1 Audited source contracts for v5 breaking changes — use interfaces to avoid pragma 0.8.15 conflicts
- [x] 4.2 Added `l2SequenceNumber()` wrapper in KailuaTournament to satisfy v5 `IDisputeGame` interface
- [x] 4.3 Updated `onlyFactoryOwner` modifier to use `DISPUTE_GAME_FACTORY.owner()` instead of `OwnableUpgradeable` cast
- [x] 4.4 Verified `Clone` (from solady) import path — works via `@solady/utils/Clone.sol`

## 5. Update Test File Imports

- [x] 5.1 Update `KailuaTest.t.sol`: Replaced concrete OP Stack imports with `@optimism/interfaces/` + mock contracts
- [x] 5.2 Adapt `KailuaTest.t.sol` to v5 API: Created `MockDisputeGameFactory` (with UUID uniqueness) and `MockOptimismPortal2` to avoid pragma conflicts
- [x] 5.3 Updated remaining test files — fixed `Propose.t.sol` `OptimismPortal2` → `IOptimismPortal2` cast with `payable()` wrapper

## 6. Update Script Imports

- [x] 6.1 Update `Deploy.s.sol`: Replace vendor imports with `@optimism/` prefixed imports, use `IOptimismPortal2`, route `setRespectedGameType` through `IAnchorStateRegistry`

## 7. Remove Vendored Files

- [x] 7.1 Delete `src/vendor/FlatOPImportV1.4.0.sol`
- [x] 7.2 Delete `src/vendor/FlatR0ImportV2.0.2.sol`
- [x] 7.3 Remove `src/vendor/` directory

## 8. Build and Test Verification

- [x] 8.1 Run `forge build` — compiles successfully (65 files), no warnings
- [x] 8.2 Run `forge test` — all 60 tests pass
- [x] 8.3 Fixed mock factory UUID uniqueness check for duplication test

## 9. CI Updates

- [x] 9.1 CI already had `submodules: recursive` in checkout step
- [ ] 9.2 Verify CI workflow runs successfully with the new submodule dependencies

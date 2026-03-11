## 1. Install Dependencies

- [x] 1.1 Install OP Stack v5.0.0 as git submodule: `forge install ethereum-optimism/optimism@op-contracts/v5.0.0 --no-commit` under `lib/optimism`
- [x] 1.2 Install RISC Zero Ethereum v3.0.1 as git submodule: `forge install risc0/risc0-ethereum@v3.0.1 --no-commit` under `lib/risc0-ethereum`
- [x] 1.3 Initialize any transitive submodules required by OP Stack and RISC Zero (OpenZeppelin, solady, etc.)

## 2. Configure Foundry

- [x] 2.1 Add remappings to `foundry.toml` for `src/`, `interfaces/`, `@risc0/`, and transitive dependencies (`@openzeppelin/contracts/`, `@openzeppelin/contracts-upgradeable/`, `@solady/`, `safe-contracts/`)
- [x] 2.2 Disabled solar linter (`lint_on_build = false`) due to src/ remapping conflict with source directory
- [x] 2.3 Verified Solidity version compatibility — kept 0.8.24, use interfaces only (no concrete v5 contracts with pragma 0.8.15)
- [x] 2.4 Update `.gitignore` if needed for new submodule artifacts (not needed — submodules tracked by git)

## 3. Update Source Contract Imports

- [x] 3.1 Update `KailuaLib.sol`: Replace vendor imports with named imports from OP Stack paths
- [x] 3.2 Update `KailuaTournament.sol`: Replace vendor imports, use `IOptimismPortal2`/`IDisputeGameFactory` interfaces
- [x] 3.3 Update `KailuaVerifier.sol`: Replace vendor imports for `ISemver`, `Duration`, `IRiscZeroVerifier`
- [x] 3.4 Update `KailuaTreasury.sol`: Replace vendor imports, add `IInitializable` import
- [x] 3.5 Update `KailuaGame.sol`: Replace vendor imports, add `IInitializable` import

## 4. Adapt Source Contracts to v5 API Changes

- [x] 4.1 Audited source contracts for v5 breaking changes — use interfaces to avoid pragma conflicts
- [x] 4.2 Added `l2SequenceNumber()` wrapper in KailuaTournament to satisfy v5 `IDisputeGame` interface
- [x] 4.3 Updated `onlyFactoryOwner` modifier to use `DISPUTE_GAME_FACTORY.owner()` instead of `OwnableUpgradeable` cast
- [x] 4.4 Verified `Clone` (from solady) import path — works via `@solady/utils/Clone.sol`

## 5. Update Test File Imports

- [x] 5.1 Update `KailuaTest.t.sol`: Replaced concrete OP Stack imports with interfaces + mock contracts
- [x] 5.2 Adapt `KailuaTest.t.sol` to v5 API: Created `MockDisputeGameFactory` and `MockOptimismPortal2` to avoid pragma conflicts
- [x] 5.3 Updated remaining test files — fixed `Propose.t.sol` `OptimismPortal2` → `IOptimismPortal2` cast

## 6. Update Script Imports

- [x] 6.1 Update `Deploy.s.sol`: Replace vendor imports, use `IOptimismPortal2`, route `setRespectedGameType` through `AnchorStateRegistry`

## 7. Remove Vendored Files

- [x] 7.1 Delete `src/vendor/FlatOPImportV1.4.0.sol`
- [x] 7.2 Delete `src/vendor/FlatR0ImportV2.0.2.sol`
- [x] 7.3 Remove `src/vendor/` directory

## 8. Build and Test Verification

- [x] 8.1 Run `forge build` — compiles successfully (64 files)
- [x] 8.2 Run `forge test` — all 60 tests pass
- [x] 8.3 Fixed mock factory UUID uniqueness check for duplication test

## 9. CI Updates

- [x] 9.1 CI already had `submodules: recursive` in checkout step
- [ ] 9.2 Verify CI workflow runs successfully with the new submodule dependencies

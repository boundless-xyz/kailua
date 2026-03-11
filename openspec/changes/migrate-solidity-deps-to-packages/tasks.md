## 1. Install Dependencies

- [ ] 1.1 Install OP Stack v5.0.0 as git submodule: `forge install ethereum-optimism/optimism@op-contracts/v5.0.0 --no-commit` under `lib/optimism`
- [ ] 1.2 Install RISC Zero Ethereum v3.0.1 as git submodule: `forge install risc0/risc0-ethereum@v3.0.1 --no-commit` under `lib/risc0-ethereum`
- [ ] 1.3 Initialize any transitive submodules required by OP Stack and RISC Zero (OpenZeppelin, solady, etc.)

## 2. Configure Foundry

- [ ] 2.1 Add remappings to `foundry.toml` for `@opstack/`, `@opstack-interfaces/`, `@risc0/`, and transitive dependencies (`@openzeppelin/contracts/`, `@openzeppelin/contracts-upgradeable/`, `@solady/`, `safe-contracts/`)
- [ ] 2.2 Add `via_ir = true` to `foundry.toml` if required by RISC Zero contracts
- [ ] 2.3 Verify Solidity version compatibility (keep 0.8.24 or bump if needed)
- [ ] 2.4 Update `.gitignore` if needed for new submodule artifacts

## 3. Update Source Contract Imports

- [ ] 3.1 Update `KailuaLib.sol`: Replace vendor imports with named imports from `@opstack/` and `@opstack-interfaces/` paths for `Timestamp` and other types used
- [ ] 3.2 Update `KailuaTournament.sol`: Replace vendor imports for `Clone`, `IDisputeGame`, `GameStatus`, `Claim`, `Hash`, `GameType`, `Timestamp`, `Duration`, `OptimismPortal2`, `DisputeGameFactory`
- [ ] 3.3 Update `KailuaVerifier.sol`: Replace vendor imports for `ISemver`, `Duration` (from OP Stack) and `IRiscZeroVerifier` (from `@risc0/`)
- [ ] 3.4 Update `KailuaTreasury.sol`: Replace vendor imports for `IInitializable`, `IDisputeGame`, `GameStatus`, `Claim`, `GameType`, `Timestamp`, `Duration`, `OptimismPortal2`, `OwnableUpgradeable`
- [ ] 3.5 Update `KailuaGame.sol`: Replace vendor imports for `IDisputeGame`, `GameStatus`, `GameType`, `Timestamp`, `Duration`, `Hash`

## 4. Adapt Source Contracts to v5 API Changes

- [ ] 4.1 Audit each source contract for OP Stack v5 breaking changes: check if any method calls on `OptimismPortal2`, `DisputeGameFactory`, `SystemConfig`, or `SuperchainConfig` have changed signatures
- [ ] 4.2 Update any references to renamed types (`OutputRoot` → `Proposal`, `l2BlockNumber` → `l2SequenceNumber`) if used
- [ ] 4.3 Update interface implementations if `IDisputeGame` has new required methods in v5
- [ ] 4.4 Verify `Clone` (from solady) import path and compatibility

## 5. Update Test File Imports

- [ ] 5.1 Update `KailuaTest.t.sol`: Replace vendor imports for `LibClone`, `DisputeGameFactory`, `OptimismPortal2`, `SystemConfig`, `SuperchainConfig`, `GameType`, `Claim`, `Duration`, `RiscZeroMockVerifier`, `ReceiptClaimLib`
- [ ] 5.2 Adapt `KailuaTest.t.sol` to v5 API: update `DisputeGameFactory` initialization, `OptimismPortal2` initialization, `SuperchainConfig` initialization, and any changed constructor signatures
- [ ] 5.3 Update remaining test files (BlobDispute, Bond, ClaimDispute, Costs, Deploy, FaultSemaphore, KZG, Propose, Reward) if they have direct vendor imports beyond what's inherited from KailuaTest

## 6. Update Script Imports

- [ ] 6.1 Update `Deploy.s.sol`: Replace vendor imports for `IDisputeGameFactory`, `OptimismPortal2`, `GameType`, `Claim`, `Duration`, `IDisputeGame` (from OP Stack) and `IRiscZeroVerifier`, `RiscZeroVerifierRouter`, `RiscZeroGroth16Verifier` (from RISC Zero)

## 7. Remove Vendored Files

- [ ] 7.1 Delete `src/vendor/FlatOPImportV1.4.0.sol`
- [ ] 7.2 Delete `src/vendor/FlatR0ImportV2.0.2.sol`
- [ ] 7.3 Remove `src/vendor/` directory if empty

## 8. Build and Test Verification

- [ ] 8.1 Run `forge build` and fix any compilation errors
- [ ] 8.2 Run `forge test` and fix any test failures
- [ ] 8.3 Iterate on remappings and import paths until all compilation and test issues are resolved

## 9. CI Updates

- [ ] 9.1 Update `.github/workflows/test.yml` to add `submodules: recursive` to the checkout step
- [ ] 9.2 Verify CI workflow runs successfully with the new submodule dependencies

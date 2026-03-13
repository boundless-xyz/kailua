## Why

The previous change (`upgradeable-kailua-verifier`) updated `Deploy.s.sol` to deploy KailuaVerifier behind an OP Stack Proxy. However, the CLI's `fast-track` command in `bin/cli/src/fast_track.rs` still deploys KailuaVerifier directly without a proxy. Both deployment paths should be consistent.

Additionally, the OP Stack `Proxy.sol` uses `pragma solidity 0.8.15` which is incompatible with the project's `0.8.24` pin. A local reimplementation is needed to produce a compilable Foundry artifact with Rust bindings.

## What Changes

- **New `src/Proxy.sol`**: ABI-compatible reimplementation of OP Stack's `Proxy.sol` at `pragma 0.8.24`, using the same EIP-1967 storage slots. This produces a full Foundry artifact for Rust binding generation.
- **New Rust binding**: Add `Proxy` to `crates/contracts/src/lib.rs` via `sol!` macro from the new artifact.
- **`Deploy.s.sol` updated**: Import `Proxy` from local `src/Proxy.sol` instead of `@optimism/src/universal/Proxy.sol`.
- **`UpgradeVerifier.s.sol` updated**: Same import change.
- **`fast_track.rs` updated**: Deploy KailuaVerifier behind a Proxy, transfer admin to the DGF owner (`factory_owner_address`), and pass the proxy address to downstream contract deployments.

## Capabilities

### New Capabilities
- `proxy-contract-wrapper`: Local Proxy.sol reimplementation compatible with project's Solidity version, producing Foundry artifacts for Rust bindings
- `fast-track-proxy`: Fast-track CLI deploys KailuaVerifier behind a Proxy with admin set to the DGF owner

### Modified Capabilities
- `proxy-deployment`: Deploy.s.sol and UpgradeVerifier.s.sol import Proxy from the local wrapper instead of OP Stack's version

## Impact

- **Contracts**: New `src/Proxy.sol`, import changes in `scripts/Deploy.s.sol` and `scripts/UpgradeVerifier.s.sol`
- **Rust crate `kailua-contracts`**: New `Proxy` binding in `lib.rs`
- **CLI binary**: `fast_track.rs` gains proxy deployment logic (~15 lines added)
- **Operators**: Fast-track deployments now automatically use a proxy; no additional flags needed

## Why

KailuaVerifier stores critical configuration (FPVM_IMAGE_ID, RISC_ZERO_VERIFIER, etc.) as immutable values baked into bytecode. When the FPVM image or verifier contract needs updating, the only option today is redeploying KailuaVerifier and all contracts that reference it. Deploying KailuaVerifier behind an OP Stack Proxy allows upgrading the implementation in-place — the proxy address stays stable, so KailuaTreasury and KailuaGame continue working without redeployment.

## What Changes

- **Deploy.s.sol** modified to deploy KailuaVerifier behind an OP Stack `Proxy` contract
  - Proxy admin defaults to `dgf.owner()`, overridable via `PROXY_ADMIN` env var
  - `RiscZeroVerifierRouter` and `RiscZeroGroth16Verifier` only deployed when `RISC_ZERO_VERIFIER` env var is not set
- **New `UpgradeVerifier.s.sol`** script that deploys a new KailuaVerifier implementation and switches the proxy to it
  - Only `PRIVATE_KEY` and `KAILUA_VERIFIER_PROXY` are required; all other params read from current implementation if not specified
- **New `fpvm-upgrade.md`** book page documenting the FPVM upgrade process under the "On-chain" section

## Capabilities

### New Capabilities
- `proxy-deployment`: Deploying KailuaVerifier behind an OP Stack Proxy with configurable admin
- `verifier-upgrade`: Upgrading a deployed KailuaVerifier proxy to a new implementation
- `fpvm-upgrade-docs`: Book documentation for the FPVM upgrade process

### Modified Capabilities

## Impact

- **Contracts**: `Deploy.s.sol` changes deployment flow for KailuaVerifier; new `UpgradeVerifier.s.sol` script added
- **Dependencies**: Imports OP Stack `Proxy.sol` from existing `@optimism/src/universal/Proxy.sol`
- **Downstream**: KailuaTreasury and KailuaGame receive the proxy address as their KailuaVerifier reference — no changes needed to those contracts
- **Operators**: New env vars (`PROXY_ADMIN` optional, `KAILUA_VERIFIER_PROXY` for upgrades); existing deployment flow is backward-compatible
- **Documentation**: New book page, SUMMARY.md updated

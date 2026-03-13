## Context

KailuaVerifier is deployed once and referenced by KailuaTreasury and KailuaGame via an immutable `KAILUA_VERIFIER` address stored in KailuaTournament. Its configuration (FPVM_IMAGE_ID, RISC_ZERO_VERIFIER, ROLLUP_CONFIG_HASH, PERMIT_DURATION, PERMIT_DELAY) is set as Solidity `immutable` values in bytecode. The `faultProofPermits` mapping is the only mutable storage.

Today, changing the FPVM image requires redeploying KailuaVerifier and all downstream contracts. The OP Stack ecosystem already uses EIP-1967 proxy patterns (`Proxy.sol`) for upgradeability.

## Goals / Non-Goals

**Goals:**
- Deploy KailuaVerifier behind an OP Stack Proxy so the implementation can be swapped without changing the address seen by KailuaTreasury/KailuaGame
- Provide a standalone upgrade script for deploying a new implementation and switching the proxy
- Document the upgrade process in the book
- Make router/groth16 deployment conditional on the `RISC_ZERO_VERIFIER` env var

**Non-Goals:**
- Upgradeability for KailuaTreasury or KailuaGame
- Changing KailuaVerifier's storage layout or converting immutables to storage variables
- Governance/timelock patterns for the proxy admin
- Automated migration of existing deployments

## Decisions

### 1. Use OP Stack's Proxy.sol (not OpenZeppelin TransparentUpgradeableProxy)

OP Stack deployments use `@optimism/src/universal/Proxy.sol` for all upgradeable contracts. Using the same pattern keeps the deployment consistent with the rest of the stack and is familiar to OP operators.

**Alternative considered:** OpenZeppelin's `TransparentUpgradeableProxy` — more widely used across the ecosystem but would introduce a different proxy pattern than the rest of the OP Stack.

### 2. Keep immutables in KailuaVerifier (no initializer pattern)

Solidity immutables are embedded in the implementation's bytecode. On `delegatecall`, the implementation's immutable values are used (read from bytecode, not storage). This means:
- Upgrading the implementation naturally changes the immutable config values
- The `faultProofPermits` mapping lives in proxy storage and carries over across upgrades
- No `initialize()` function or storage variable migration needed

**Alternative considered:** Converting immutables to storage + initializer pattern — adds gas cost on every read, requires storage layout management, and adds an `initialize()` function that must be called atomically with `upgradeToAndCall()`. Unnecessary complexity since there's only one KailuaVerifier instance.

### 3. Proxy admin defaults to DisputeGameFactory owner

The DGF owner is the natural authority for the OP Stack deployment. `dgf.owner()` is available via the `OwnableUpgradeable` inheritance on `DisputeGameFactory`. An optional `PROXY_ADMIN` env var allows overriding this for deployments where a different admin is desired.

During deployment, the deployer is the initial proxy admin (to call `upgradeTo`), then admin is transferred to the resolved admin address.

### 4. Upgrade script reads missing params from current implementation

The upgrade script requires only `PRIVATE_KEY` and `KAILUA_VERIFIER_PROXY`. All KailuaVerifier constructor params (`RISC_ZERO_VERIFIER`, `FPVM_IMAGE_ID`, `ROLLUP_CONFIG_HASH`, `PERMIT_DURATION`, `PERMIT_DELAY`) default to the current implementation's values when not set in env. This means operators only specify what's changing.

Reading is done via `KailuaVerifier(proxy).FPVM_IMAGE_ID()` etc. — these calls go through the proxy to the current implementation.

### 5. Conditional router/groth16 deployment

`_6_1_proofVerification()` currently deploys `RiscZeroVerifierRouter` and `RiscZeroGroth16Verifier` unconditionally but doesn't use them (it reads `riscZeroVerifier` from env). The fix: only deploy them when `RISC_ZERO_VERIFIER` is not set, and use the deployed router as the verifier in that case.

Use `vm.envOr("RISC_ZERO_VERIFIER", address(0))` to detect presence.

## Risks / Trade-offs

**[Immutable values read from wrong implementation during upgrade tx]** → Not a risk. `upgradeTo` is an admin-only call that doesn't invoke KailuaVerifier logic. The next `delegatecall` after the upgrade reads the new implementation's immutables.

**[Proxy adds gas overhead per call]** → Minimal. EIP-1967 proxy adds ~2600 gas per `delegatecall`. KailuaVerifier calls (proof verification, permit operations) are already expensive, so the overhead is negligible.

**[Admin key compromise allows malicious upgrade]** → Mitigated by defaulting to DGF owner, which in production OP Stack deployments is a multisig/Safe. Documentation should note that the proxy admin should be secured with the same rigor as the DGF owner.

**[Storage collision between proxy and implementation]** → Not a risk. KailuaVerifier's only storage is a `mapping(bytes32 => FaultProofPermit[])` at slot 0. The Proxy stores admin and implementation addresses in EIP-1967 slots, which are computed from hashes and won't collide with slot 0.

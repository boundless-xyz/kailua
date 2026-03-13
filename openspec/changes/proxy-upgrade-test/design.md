## Context

KailuaVerifier is now deployed behind an EIP-1967 Proxy (`src/Proxy.sol`). The existing test base class `KailuaTest` deploys KailuaVerifier directly. We need a test that exercises the proxy upgrade path and verifies that immutable config changes in the new implementation are visible through the proxy to all downstream contracts.

Key behavior to validate:
- Immutables (FPVM_IMAGE_ID, etc.) come from the implementation's bytecode, not proxy storage
- The `faultProofPermits` mapping lives in proxy storage and survives upgrades
- Downstream contracts (Treasury, Game) hold the proxy address and automatically see new implementation values
- Only the proxy admin can call `upgradeTo`

## Goals / Non-Goals

**Goals:**
- Test initial deployment behind proxy with parameter reads
- Test upgrade to new implementation with different parameters
- Test that Treasury/Game see updated parameters through the stable proxy address
- Test that permits in proxy storage survive an upgrade
- Test that non-admin cannot call upgradeTo

**Non-Goals:**
- Testing KailuaVerifier business logic (already covered by existing tests)
- Testing the Proxy contract itself in isolation
- Testing the UpgradeVerifier.s.sol script

## Decisions

### 1. Extend KailuaTest and override verifier setup

The test inherits from `KailuaTest`, calls `super.setUp()` to get mocks, then wraps the verifier in a Proxy and reassigns the `verifier` field. This way `deployKailua()` uses the proxy address, and Treasury/Game store it as their immutable `KAILUA_VERIFIER`.

### 2. Proxy admin is the test contract

Using `address(this)` as the proxy admin matches the pattern in `KailuaTest` where the test contract owns everything. This allows direct `upgradeTo` calls in tests.

### 3. Read parameters via fallback, not admin path

The Proxy's `proxyCallIfNotAdmin` modifier means admin calls to explicit Proxy functions (`upgradeTo`, `changeAdmin`) execute on the proxy. But calls to unknown selectors (like `FPVM_IMAGE_ID()`) go through `fallback()` → `_doProxyCall()` for all callers. So reading KailuaVerifier parameters through the proxy works from any address including admin.

### 4. Two distinct parameter sets for before/after

Use clearly different values so assertions are unambiguous:
- **v1**: FPVM_IMAGE_ID = 0x01..01, PERMIT_DURATION = 200, PERMIT_DELAY = 100
- **v2**: FPVM_IMAGE_ID = 0x02..02, PERMIT_DURATION = 400, PERMIT_DELAY = 200

## Risks / Trade-offs

**[Permit survival test depends on acquiring a permit]** → Acquiring a permit requires a deployed Treasury with proposals. The test will use the full `deployKailua()` flow to get a working system, acquire a permit, then upgrade and verify the permit persists.

## Why

The proxy deployment and upgrade flow for KailuaVerifier has no test coverage. We need a Solidity test that verifies the full lifecycle: deploy behind proxy, upgrade to a new implementation with different parameters, and confirm downstream contracts (KailuaTreasury, KailuaGame) see the updated values through the proxy.

## What Changes

- **New `test/ProxyUpgrade.t.sol`**: Foundry test that extends `KailuaTest`, deploys KailuaVerifier behind a Proxy, performs an upgrade, and asserts parameter changes propagate through the system.

## Capabilities

### New Capabilities
- `proxy-upgrade-test`: Solidity test for the KailuaVerifier proxy upgrade lifecycle

### Modified Capabilities

## Impact

- **Tests**: New test file `test/ProxyUpgrade.t.sol`
- **No production code changes**

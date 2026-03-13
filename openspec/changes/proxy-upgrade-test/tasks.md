## 1. Create ProxyUpgrade.t.sol

- [x] 1.1 Create `test/ProxyUpgrade.t.sol` extending `KailuaTest` with setUp that wraps verifier in Proxy, reassigns `verifier` field, and calls `deployKailua()`
- [x] 1.2 Implement `testParametersReadThroughProxy` — assert all 5 immutables are readable through the proxy
- [x] 1.3 Implement `testUpgradeChangesParameters` — deploy new impl with different params, upgrade proxy, assert all 5 values changed
- [x] 1.4 Implement `testTreasurySeesUpgradedParameters` — after upgrade, read FPVM_IMAGE_ID through `treasury.KAILUA_VERIFIER()` and assert it matches new value
- [x] 1.5 Implement `testNonAdminCannotUpgrade` — prank as non-admin, call upgradeTo, expect revert
- [x] 1.6 Implement `testPermitsSurviveUpgrade` — acquire a permit pre-upgrade, upgrade, verify permit still readable

## 2. Verify

- [x] 2.1 Run `forge test --match-contract ProxyUpgrade` and confirm all tests pass

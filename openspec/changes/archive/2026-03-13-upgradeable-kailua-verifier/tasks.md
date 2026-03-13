## 1. Modify Deploy.s.sol

- [x] 1.1 Add `Proxy` import from `@optimism/src/universal/Proxy.sol`
- [x] 1.2 Change `riscZeroVerifier` state var to use `vm.envOr("RISC_ZERO_VERIFIER", address(0))` and store as address
- [x] 1.3 Add `proxyAdmin` resolution: `vm.envOr("PROXY_ADMIN", dgf.owner())`
- [x] 1.4 Update `_6_1_proofVerification()`: conditionally deploy router/groth16 only when `riscZeroVerifier == address(0)`, setting `riscZeroVerifier` to the router address
- [x] 1.5 Update `_6_1_proofVerification()`: deploy KailuaVerifier implementation, deploy Proxy with deployer as admin, call `upgradeTo`, call `changeAdmin(proxyAdmin)`, return `KailuaVerifier(address(proxy))`

## 2. Create UpgradeVerifier.s.sol

- [x] 2.1 Create `UpgradeVerifier.s.sol` in `crates/contracts/foundry/scripts/` with required env vars (`PRIVATE_KEY`, `KAILUA_VERIFIER_PROXY`)
- [x] 2.2 Implement param resolution: for each of `RISC_ZERO_VERIFIER`, `FPVM_IMAGE_ID`, `ROLLUP_CONFIG_HASH`, `PERMIT_DURATION`, `PERMIT_DELAY`, use env var if set, otherwise read from `KailuaVerifier(proxy)`
- [x] 2.3 Implement `run()`: deploy new KailuaVerifier with resolved params, call `Proxy(proxy).upgradeTo(newImpl)`

## 3. Book Documentation

- [x] 3.1 Create `book/src/fpvm-upgrade.md` with overview, architecture, prerequisites, upgrade command, and verification sections
- [x] 3.2 Update `book/src/SUMMARY.md` to link "FPVM Upgrade" under "On-chain"

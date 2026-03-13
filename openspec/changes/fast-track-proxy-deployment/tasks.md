## 1. Create local Proxy.sol wrapper

- [ ] 1.1 Create `src/Proxy.sol` with `pragma 0.8.24`, reimplementing OP Stack Proxy with inlined EIP-1967 constants
- [ ] 1.2 Verify `forge build` produces `out/Proxy.sol/Proxy.json` with bytecode (src/ files are compiled by default)

## 2. Update Solidity imports

- [ ] 2.1 Update `scripts/Deploy.s.sol` to import `Proxy` from `"../src/Proxy.sol"` instead of `"@optimism/src/universal/Proxy.sol"`
- [ ] 2.2 Update `scripts/UpgradeVerifier.s.sol` to import `Proxy` from `"../src/Proxy.sol"` instead of `"@optimism/src/universal/Proxy.sol"`
- [ ] 2.3 Verify `forge build scripts/Deploy.s.sol scripts/UpgradeVerifier.s.sol` compiles cleanly (scripts need explicit paths)

## 3. Add Rust binding

- [ ] 3.1 Add `Proxy` sol! binding in `crates/contracts/src/lib.rs` referencing `foundry/out/Proxy.sol/Proxy.json`

## 4. Update fast_track.rs

- [ ] 4.1 Import `Proxy` from `kailua_contracts`
- [ ] 4.2 After KailuaVerifier deploy (line ~288), add: deploy Proxy with deployer as admin, call `upgradeTo(verifier_impl)`, call `changeAdmin(factory_owner_address)`
- [ ] 4.3 Pass proxy address (instead of verifier_impl address) to KailuaTreasury deploy

## 5. Verify

- [ ] 5.1 Run `cargo build` for `kailua-cli` to confirm Rust compilation

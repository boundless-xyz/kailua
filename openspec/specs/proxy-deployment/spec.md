### Requirement: Deploy KailuaVerifier behind OP Stack Proxy
The `_6_1_proofVerification()` function in `Deploy.s.sol` SHALL deploy KailuaVerifier as an implementation behind a `Proxy` contract imported from local `src/Proxy.sol` (not from `@optimism/src/universal/Proxy.sol`). The function SHALL return `KailuaVerifier(address(proxy))` so downstream deployment steps receive the proxy address.

#### Scenario: Standard deployment with RISC_ZERO_VERIFIER set
- **WHEN** `Deploy.s.sol` is executed with `RISC_ZERO_VERIFIER` env var set
- **THEN** the script deploys a KailuaVerifier implementation using the provided verifier address, deploys a Proxy with the deployer as initial admin, calls `proxy.upgradeTo(implementation)`, transfers proxy admin, and returns the proxy address cast as KailuaVerifier

#### Scenario: Deployment without RISC_ZERO_VERIFIER
- **WHEN** `Deploy.s.sol` is executed without `RISC_ZERO_VERIFIER` env var
- **THEN** the script deploys `RiscZeroVerifierRouter` and `RiscZeroGroth16Verifier`, registers the groth16 verifier in the router, and uses the router address as the RISC Zero verifier for the KailuaVerifier implementation

### Requirement: Proxy admin defaults to DGF owner
The proxy admin SHALL default to `dgf.owner()` unless the `PROXY_ADMIN` env var is set.

#### Scenario: Default admin (no PROXY_ADMIN env var)
- **WHEN** `PROXY_ADMIN` env var is not set
- **THEN** the proxy admin is set to `dgf.owner()`

#### Scenario: Override admin (PROXY_ADMIN env var set)
- **WHEN** `PROXY_ADMIN` env var is set to an address
- **THEN** the proxy admin is set to that address

### Requirement: Admin transfer during deployment
During deployment, the deployer MUST be the initial proxy admin to call `upgradeTo`. After setting the implementation, the script SHALL call `proxy.changeAdmin(resolvedAdmin)` to transfer admin to the resolved admin address.

#### Scenario: Admin transfer sequence
- **WHEN** the proxy is deployed and implementation is set
- **THEN** `proxy.changeAdmin()` is called with the resolved admin address before the function returns

### Requirement: Conditional router and groth16 deployment
`RiscZeroVerifierRouter` and `RiscZeroGroth16Verifier` SHALL only be deployed when `RISC_ZERO_VERIFIER` is not provided via env var.

#### Scenario: Router not deployed when verifier provided
- **WHEN** `RISC_ZERO_VERIFIER` env var is set
- **THEN** no `RiscZeroVerifierRouter` or `RiscZeroGroth16Verifier` contracts are deployed

#### Scenario: Router deployed when verifier not provided
- **WHEN** `RISC_ZERO_VERIFIER` env var is not set
- **THEN** `RiscZeroVerifierRouter` and `RiscZeroGroth16Verifier` are deployed, groth16 is registered in the router, and the router is used as the verifier

### Requirement: UpgradeVerifier uses local Proxy import
`UpgradeVerifier.s.sol` SHALL import `Proxy` from `../src/Proxy.sol` instead of `@optimism/src/universal/Proxy.sol`.

#### Scenario: UpgradeVerifier compiles
- **WHEN** `forge build` is run
- **THEN** `UpgradeVerifier.s.sol` compiles without errors using the local Proxy import

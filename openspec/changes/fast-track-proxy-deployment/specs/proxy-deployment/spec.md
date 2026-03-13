## MODIFIED Requirements

### Requirement: Deploy KailuaVerifier behind OP Stack Proxy
The `_6_1_proofVerification()` function in `Deploy.s.sol` SHALL deploy KailuaVerifier as an implementation behind a `Proxy` contract imported from local `src/Proxy.sol` (not from `@optimism/src/universal/Proxy.sol`). The function SHALL return `KailuaVerifier(address(proxy))` so downstream deployment steps receive the proxy address.

#### Scenario: Standard deployment with RISC_ZERO_VERIFIER set
- **WHEN** `Deploy.s.sol` is executed with `RISC_ZERO_VERIFIER` env var set
- **THEN** the script deploys a KailuaVerifier implementation using the provided verifier address, deploys a Proxy with the deployer as initial admin, calls `proxy.upgradeTo(implementation)`, transfers proxy admin, and returns the proxy address cast as KailuaVerifier

#### Scenario: Deployment without RISC_ZERO_VERIFIER
- **WHEN** `Deploy.s.sol` is executed without `RISC_ZERO_VERIFIER` env var
- **THEN** the script deploys `RiscZeroVerifierRouter` and `RiscZeroGroth16Verifier`, registers the groth16 verifier in the router, and uses the router address as the RISC Zero verifier for the KailuaVerifier implementation

### Requirement: UpgradeVerifier uses local Proxy import
`UpgradeVerifier.s.sol` SHALL import `Proxy` from `../src/Proxy.sol` instead of `@optimism/src/universal/Proxy.sol`.

#### Scenario: UpgradeVerifier compiles
- **WHEN** `forge build` is run
- **THEN** `UpgradeVerifier.s.sol` compiles without errors using the local Proxy import

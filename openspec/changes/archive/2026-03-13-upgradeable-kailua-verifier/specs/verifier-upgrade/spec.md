## ADDED Requirements

### Requirement: UpgradeVerifier script deploys new implementation and switches proxy
`UpgradeVerifier.s.sol` SHALL deploy a new KailuaVerifier contract and call `Proxy(KAILUA_VERIFIER_PROXY).upgradeTo(newImplementation)`.

#### Scenario: Upgrade with all params specified
- **WHEN** the script is run with `KAILUA_VERIFIER_PROXY`, `PRIVATE_KEY`, and all KailuaVerifier constructor params set
- **THEN** a new KailuaVerifier is deployed with the specified params and the proxy implementation is switched to it

#### Scenario: Upgrade with only required params
- **WHEN** the script is run with only `KAILUA_VERIFIER_PROXY` and `PRIVATE_KEY`
- **THEN** the script reads `RISC_ZERO_VERIFIER`, `FPVM_IMAGE_ID`, `ROLLUP_CONFIG_HASH`, `PERMIT_DURATION`, and `PERMIT_DELAY` from the current proxy implementation and deploys a new KailuaVerifier with those same values

### Requirement: Missing params read from current implementation
For each KailuaVerifier constructor param (`RISC_ZERO_VERIFIER`, `FPVM_IMAGE_ID`, `ROLLUP_CONFIG_HASH`, `PERMIT_DURATION`, `PERMIT_DELAY`), the script SHALL use the env var value if set, otherwise read the current value from the proxy.

#### Scenario: Partial param override (only FPVM_IMAGE_ID changed)
- **WHEN** only `FPVM_IMAGE_ID` is set in env along with the required params
- **THEN** the new implementation uses the provided `FPVM_IMAGE_ID` and reads all other params from the current proxy

#### Scenario: Read current values via proxy
- **WHEN** a param is not set in env
- **THEN** the script reads it by calling the corresponding getter on `KailuaVerifier(KAILUA_VERIFIER_PROXY)` (e.g., `.FPVM_IMAGE_ID()`, `.RISC_ZERO_VERIFIER()`)

### Requirement: Caller must be proxy admin
The upgrade transaction MUST be sent by the proxy admin. The script SHALL use `PRIVATE_KEY` to sign the transaction.

#### Scenario: Non-admin caller
- **WHEN** the `PRIVATE_KEY` does not correspond to the proxy admin
- **THEN** the `upgradeTo` call reverts

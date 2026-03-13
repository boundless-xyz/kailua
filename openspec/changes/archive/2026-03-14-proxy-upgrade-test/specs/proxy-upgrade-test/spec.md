## ADDED Requirements

### Requirement: Parameters readable through proxy
After deploying KailuaVerifier behind a Proxy and setting the implementation, all immutable parameters SHALL be readable through the proxy address.

#### Scenario: Read FPVM_IMAGE_ID through proxy
- **WHEN** KailuaVerifier is deployed behind a Proxy and `upgradeTo` is called
- **THEN** `KailuaVerifier(proxyAddress).FPVM_IMAGE_ID()` returns the implementation's image ID

#### Scenario: Read all immutables through proxy
- **WHEN** KailuaVerifier is deployed behind a Proxy
- **THEN** `RISC_ZERO_VERIFIER`, `ROLLUP_CONFIG_HASH`, `PERMIT_DURATION`, and `PERMIT_DELAY` are all readable through the proxy

### Requirement: Upgrade changes parameters
After upgrading to a new KailuaVerifier implementation with different constructor parameters, all reads through the proxy SHALL return the new values.

#### Scenario: FPVM_IMAGE_ID changes after upgrade
- **WHEN** a new KailuaVerifier with a different FPVM_IMAGE_ID is deployed and `proxy.upgradeTo(newImpl)` is called
- **THEN** `KailuaVerifier(proxyAddress).FPVM_IMAGE_ID()` returns the new image ID

#### Scenario: All immutables change after upgrade
- **WHEN** a new KailuaVerifier with all-different parameters is deployed and the proxy is upgraded
- **THEN** all five immutable getters return the new values

### Requirement: Downstream contracts see upgraded parameters
KailuaTreasury and KailuaGame hold the proxy address as their immutable `KAILUA_VERIFIER`. After an upgrade, reading through that reference SHALL return the new implementation's values.

#### Scenario: Treasury sees new FPVM_IMAGE_ID
- **WHEN** the proxy is upgraded to a new implementation
- **THEN** `KailuaVerifier(treasury.KAILUA_VERIFIER()).FPVM_IMAGE_ID()` returns the new image ID

### Requirement: Proxy storage survives upgrade
The `faultProofPermits` mapping lives in proxy storage. Permits acquired before an upgrade SHALL be readable after the upgrade.

#### Scenario: Permit persists after upgrade
- **WHEN** a fault proof permit is acquired, then the proxy is upgraded to a new implementation
- **THEN** the permit is still readable via `faultProofPermits()` on the proxy

### Requirement: Non-admin cannot upgrade
Only the proxy admin SHALL be able to call `upgradeTo`. Calls from other addresses SHALL be proxied to the implementation, which does not have `upgradeTo`, causing a revert.

#### Scenario: Non-admin upgradeTo reverts
- **WHEN** an address that is not the proxy admin calls `upgradeTo` on the proxy
- **THEN** the call reverts

### Requirement: fast_track deploys KailuaVerifier behind a Proxy
The `fast_track` function in `fast_track.rs` SHALL deploy an EIP-1967 Proxy after deploying the KailuaVerifier implementation, set the implementation via `upgradeTo`, transfer admin to the DGF owner, and pass the proxy address to downstream contract deployments.

#### Scenario: Standard fast-track deployment
- **WHEN** `kailua-cli fast-track` is executed
- **THEN** a Proxy is deployed with the deployer as initial admin, `upgradeTo(verifier_impl)` is called, `changeAdmin(factory_owner_address)` is called, and the proxy address is used as `verifier_contract_address` for KailuaTreasury deployment

#### Scenario: Proxy admin is DGF owner
- **WHEN** the DGF owner is an EOA
- **THEN** the proxy admin is transferred to that EOA address

#### Scenario: Proxy admin is Safe
- **WHEN** the DGF owner is a Safe contract
- **THEN** the proxy admin is transferred to the Safe contract address

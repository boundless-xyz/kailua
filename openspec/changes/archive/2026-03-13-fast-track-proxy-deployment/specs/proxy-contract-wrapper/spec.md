## ADDED Requirements

### Requirement: Local Proxy.sol at pragma 0.8.24
A `src/Proxy.sol` file SHALL exist with `pragma solidity 0.8.24` that is ABI-compatible with the OP Stack `Proxy.sol`. It SHALL use the same EIP-1967 storage slots for admin and implementation.

#### Scenario: Compilation produces full artifact
- **WHEN** `forge build` is run
- **THEN** `out/Proxy.sol/Proxy.json` is produced with `abi` and `bytecode` fields

#### Scenario: ABI compatibility with OP Stack Proxy
- **WHEN** the ABI of `src/Proxy.sol` is compared to the OP Stack `Proxy.sol`
- **THEN** the function signatures, event signatures, and constructor signature are identical

### Requirement: Rust binding for Proxy
`crates/contracts/src/lib.rs` SHALL contain a `sol!` binding for `Proxy` referencing the Foundry artifact at `foundry/out/Proxy.sol/Proxy.json`.

#### Scenario: Proxy binding available in Rust
- **WHEN** `kailua_contracts` is compiled
- **THEN** `Proxy::deploy_builder()`, `Proxy::new()`, and methods `upgradeTo`, `changeAdmin`, `admin` are available

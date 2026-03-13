## Context

The `upgradeable-kailua-verifier` change introduced proxy deployment in `Deploy.s.sol` but imported `Proxy` from `@optimism/src/universal/Proxy.sol` (`pragma solidity 0.8.15`). This works for script compilation (Forge handles multi-version deps transitively) but:

1. Does not produce a standalone Foundry artifact — no `out/Proxy.sol/Proxy.json` with bytecode
2. Cannot be referenced from `crates/contracts/src/lib.rs` for Rust bindings
3. The `fast_track.rs` CLI needs Rust bindings (`deploy_builder`, `upgradeTo`, `changeAdmin`) to deploy and manage the proxy

## Goals / Non-Goals

**Goals:**
- Create a local `src/Proxy.sol` that is ABI-compatible with OP Stack's Proxy, compiled under `0.8.24`, producing a full artifact
- Add Rust bindings for Proxy via `sol!` macro in `lib.rs`
- Update `Deploy.s.sol` and `UpgradeVerifier.s.sol` to import from the local wrapper
- Update `fast_track.rs` to deploy KailuaVerifier behind a Proxy with admin transferred to the DGF owner

**Non-Goals:**
- Changing the EIP-1967 storage layout or proxy behavior
- Adding ProxyAdmin contract support
- Modifying the upgrade script (UpgradeVerifier.s.sol) logic — only the import path changes

## Decisions

### 1. Reimplement Proxy.sol locally at pragma 0.8.24

The OP Stack `Proxy.sol` uses `pragma solidity =0.8.15` and imports `Constants.sol` for EIP-1967 slot values. The project pins `solc_version = "0.8.24"` in `foundry.toml`. Instead of enabling multi-version compilation or modifying the Forge config, we reimplement the contract with inlined EIP-1967 constants.

The reimplementation is byte-for-byte ABI-compatible with the OP Stack version: same constructor signature, same function signatures, same events, same storage slots.

**Alternative considered:** Enabling `auto_detect_solc` in `foundry.toml` — rejected because it changes global compilation behavior and may pull in unwanted solver versions for all OP Stack transitive dependencies.

**Alternative considered:** Embedding raw bytecode in Rust via `sol!(bytecode = "0x...")` — rejected by user; Foundry artifact is preferred.

### 2. Proxy admin defaults to DGF owner in fast_track.rs

In `fast_track.rs`, the DGF owner is already resolved as `factory_owner_address` (line 207-214). This is either an EOA or a Safe. The proxy admin is transferred to this address after deployment, matching the behavior in `Deploy.s.sol`.

The deployment uses `deployer_provider` (the deployer wallet) as initial admin, calls `upgradeTo`, then calls `changeAdmin(factory_owner_address)`.

### 3. Proxy bindings via sol! macro from Foundry artifact

Following the existing pattern in `crates/contracts/src/lib.rs`, add:
```rust
sol!(
    #[sol(rpc)]
    Proxy,
    "foundry/out/Proxy.sol/Proxy.json"
);
```
This provides `Proxy::deploy_builder()`, `Proxy::new()`, `proxy.upgradeTo()`, `proxy.changeAdmin()`.

## Risks / Trade-offs

**[Local Proxy.sol diverges from OP Stack version]** → The contract is simple (~120 lines of assembly), uses standard EIP-1967 slots, and is pinned to the OP Stack behavior. The risk of divergence is low since the proxy pattern is stable. A comment in the file references the OP Stack original.

**[ABI compatibility]** → The function signatures and events are identical. The only difference is the inlined constants vs the `Constants.sol` import. Storage layout is identical (EIP-1967 standard slots).

# OP Stack Devnet Upgrade Report

## Summary

This change upgrades Kailua's local devnet workflow from the legacy OP Stack `v1.9.1` setup to a Kurtosis-based OP Stack `v1.16.7` setup using prebuilt artifacts and local containers.

The public `just devnet-*` interface in this repository remains intact, but the implementation now uses:

- `optimism` at tag `v1.16.7`
- a pinned local checkout of `ethpandaops/optimism-package`
- a local Kurtosis enclave named `kailua-devnet`
- a rendered descriptor at `.localtestdata/kurtosis-devnet.json`

The upgraded workflow was verified by running the devnet integration tests in `bin/cli/tests/devnet.rs` successfully.

## Goals

- Keep Kailua's existing `just devnet-*` commands stable.
- Move devnet bring-up to OP `v1.16.7`.
- Use prebuilt OP images instead of building OP components locally.
- Keep everything local to the developer machine and local containers.
- Make the integration tests work against dynamic Kurtosis endpoints instead of hardcoded ports and keys.

## What Changed

### 1. Devnet bring-up moved to Kurtosis

`just devnet-up` now:

- fetches `optimism` at `v1.16.7`
- fetches a pinned `optimism-package` checkout
- patches the local package for compatibility
- runs `kurtosis run` locally in the `kailua-devnet` enclave
- renders a stable descriptor to `.localtestdata/kurtosis-devnet.json`

This keeps all OP services local. The only networked behavior is normal source/image fetching when dependencies are not already present locally.

### 2. Devnet configuration now uses a current OP package layout

`kurtosis.yaml` was rewritten around the current package schema and pinned to prebuilt images:

- `op-node:v1.16.7`
- `op-geth:v1.101609.1`
- `op-batcher:v1.16.4`
- `op-proposer:v1.16.0`
- `op-challenger:v1.9.0`
- `op-deployer:v0.6.0-rc.3`

It also disables optional extras such as observability and faucet services to keep the local devnet smaller.

### 3. Descriptor-driven defaults replaced hardcoded ports and keys

The repo now treats `.localtestdata/kurtosis-devnet.json` as the source of truth for:

- L1 RPC
- L1 beacon RPC
- L2 RPC
- op-node RPC
- deployer, owner, guardian, proposer, validator, and fault proposer keys

This is used both by shell recipes and by the Rust integration tests.

### 4. Integration tests were updated for dynamic devnet state

`bin/cli/tests/devnet.rs` now:

- invokes the repository's `just devnet-*` commands directly
- loads the rendered descriptor
- waits for RPC and beacon readiness instead of sleeping a fixed amount
- resolves wallets by alias from the descriptor

This removed assumptions about fixed localhost ports and static private keys.

### 5. Sync logic was adjusted for local Kurtosis finality behavior

The upgraded local devnet exposed an important behavioral difference: on Kurtosis, the op-node safe head can advance significantly ahead of the finalized head for long enough to stall Kailua's test flow.

To fix that, the sync provider now prefers safe L2 outputs in `devnet` mode whenever they are ahead of finalized outputs. This was the key runtime fix that made the upgraded tests complete reliably.

## Compatibility Fixes Applied

The OP package itself needed a small local compatibility patch layer:

- observability helpers now no-op cleanly when observability is disabled
- `operatorFeeVaultRecipient` is injected where Kailua expects it
- op-node is given `--rollup.l1-chain-config=/network-configs/l1-chain-config.json`
- the generated L1 chain config removes `terminalTotalDifficultyPassed`

Those changes are applied automatically to the local `optimism-package` checkout by `scripts/patch-optimism-package.sh`.

There is also a local watchdog wrapper around `kurtosis run`:

- if package upload starts but execution never begins, the wrapper terminates the run
- `just devnet-up` then restarts the Kurtosis engine and retries once for known transport failures

This addressed intermittent local engine-side failures seen during package upload / startup.

## Verification

The following commands were run successfully on the upgraded setup:

```bash
just devnet-up
cargo test -p kailua-cli -F devnet --test devnet prover -- --nocapture
cargo test -p kailua-cli -F devnet --test devnet proposer_validator -- --nocapture
```

Observed result:

- `just devnet-up` completed and produced `.localtestdata/kurtosis-devnet.json`
- `prover` passed
- `proposer_validator` passed

## Learnings

### 1. "Uploading package" in Kurtosis is still local

The `kurtosis run` flow uploads the local package to the local Kurtosis engine. That step can look remote in logs, but it is still part of local execution. It does not mean the devnet is being deployed to remote infrastructure.

### 2. OP `v1.16.7` is not a drop-in replacement for the old local flow

The move from the old `make -C optimism devnet-*` flow to the modern Kurtosis flow changes:

- service naming
- endpoint discovery
- wallet sourcing
- readiness behavior
- package schema expectations

The upgrade required adapting Kailua to those changes rather than just changing image tags.

### 3. The local devnet should not rely on finalized head progress

For this local test environment, finalized outputs lagged enough to stall the workflow. Safe head progress was the right source of truth for `devnet` mode.

This is a local testing concern, not necessarily a production behavior change.

### 4. `op-deployer` artifact behavior mattered

The original plan assumed tag-based artifact locators for contracts. In practice, `op-deployer:v0.6.0-rc.3` behaved correctly with `embedded` locators in this setup, so the final config keeps that approach.

### 5. A rendered descriptor is the simplest stable interface

Kurtosis service ports are dynamic. Rendering a Kailua-owned descriptor after bring-up turned out to be the cleanest way to:

- keep the existing `just` UX
- make tests deterministic
- avoid hardcoding localhost ports
- centralize wallet and endpoint discovery

## Files Touched

Main implementation files:

- `justfile`
- `kurtosis.yaml`
- `scripts/patch-optimism-package.sh`
- `scripts/render-devnet-descriptor.py`
- `scripts/run-kurtosis-devnet.py`
- `scripts/devnet-env.sh`
- `bin/cli/tests/devnet.rs`
- `crates/sync/src/provider/optimism.rs`
- `crates/sync/src/agent.rs`
- `crates/proposer/src/propose.rs`
- `bin/cli/src/demo.rs`
- `bin/cli/src/fast_track.rs`

Documentation updates:

- `README.md`
- `book/src/quickstart.md`

## Operational Notes

- Run `just devnet-up` before using descriptor-backed devnet commands.
- Use `just devnet-down` to remove the enclave.
- Use `just devnet-clean` to remove the enclave, descriptor, and `devnet.log`.
- If Docker images are missing locally, Docker may pull them once from the configured registries.
- The integration tests are long-running and may take several minutes each.

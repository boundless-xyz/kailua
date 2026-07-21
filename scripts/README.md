# Devnet & Build Scripts

Support scripts for the local Kurtosis devnet and the vendored guest build.
All are invoked through `justfile` recipes; direct invocation is rarely needed.
See the [Testing Tools](../book/src/testing.md) chapter for the devnet workflow.

## Devnet lifecycle

- `devnet-fetch.sh` — Clones and pins the `optimism` monorepo (v1.16.7) and the
  `ethpandaops/optimism-package` Kurtosis package into `devnet/`, then applies the local patch via
  `patch-optimism-package.sh`. Idempotent; re-run to restore the pinned commits.
- `patch-optimism-package.sh` — Applies `optimism-package-v1.16.7.patch` to a Kurtosis package
  checkout after verifying it is at the expected commit.
- `devnet-up.sh` — Launches the `kailua-devnet` enclave via `run-kurtosis-devnet.py` using
  `kurtosis.yaml`. On a package-upload stall (exit code 75) it restarts the Kurtosis engine and
  retries once. Finishes by rendering `devnet/kurtosis-devnet.json` with
  `render-devnet-descriptor.py`.
- `devnet-down.sh` — Removes the enclave.
- `devnet-clean.sh` — Removes the enclave and deletes the descriptor, log, and data directories
  under `devnet/`.
- `run-kurtosis-devnet.py` — Wrapper around `kurtosis run` that tails the launch log and aborts
  with exit code 75 if output stalls beyond `--stall-timeout-secs`.
- `render-devnet-descriptor.py` — Inspects a running enclave and writes a JSON descriptor of its
  service endpoints and prefunded wallets.
- `devnet-env.sh` — Sourced by the `devnet-*` justfile recipes. `devnet_load_env` reads the
  descriptor and exports `DEVNET_L1_RPC`, `DEVNET_L2_RPC`, `DEVNET_OP_NODE_RPC`, wallet keys, etc.;
  `devnet_resolve` prefers an explicit recipe argument over the descriptor value.

## EigenDA variant

- `devnet-up-eigenda.sh` — Same flow as `devnet-up.sh` for the `kailua-eigenda-devnet` enclave
  using `kurtosis-eigenda.yaml`. First builds a local `kailua/eigenda-proxy:devnet` Docker image
  from a fresh `Layr-Labs/eigenda-proxy` checkout.
- `devnet-clean-eigenda.sh` — Enclave and artifact cleanup for the EigenDA devnet.
- `eigenda-proxy-encoded-shim.py` — Standalone HTTP shim that forwards EigenDA proxy GET requests
  and re-encodes successful payload responses when `return_encoded_payload=true` is requested, for
  fronting proxy versions that cannot serve encoded payloads themselves.

## Guest build

- `stage-nut-bundles.sh` — Stages kona-hardforks' `op-core` NUT-bundle JSON files into the vendored
  tree so its `build.rs` codegen can find them after `cargo vendor` (which only copies a crate's
  own directory). Part of `just vendor`; see the header comment for details.

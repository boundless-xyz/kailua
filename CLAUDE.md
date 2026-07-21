# Kailua

Kailua is a ZK fault proof system for OP Stack rollups. It replaces optimistic bisection games
with RISC Zero zkVM proofs of [kona](https://github.com/ethereum-optimism/optimism) (the Rust OP
Stack state-transition client), supporting both fault proving (disproving bad proposals) and
validity proving (fast-finalizing good ones). Data availability can come from Ethereum blobs,
EigenDA (`eigen` feature, via hokulea), or Celestia (`celestia` feature, via hana).

Operator and protocol documentation lives in `book/` (mdbook, published to GitHub Pages). Read
`book/src/design.md` before changing protocol logic, and `book/src/dependency-upgrades.md` +
`book/src/fpvm-upgrade.md` before touching dependencies. Security audits are in `audits/`.

## Repository map

This repo contains **four separate cargo workspaces**:

- The host workspace (root `Cargo.toml`): `bin/*`, `build/*`, `crates/*`.
- Three guest workspaces: `build/risczero/{kona,hokulea,hana}` — one per FPVM guest program,
  each with its **own `Cargo.lock` and `rust-toolchain.toml`**. They path-depend back on
  `crates/{kona,hokulea,hana}`.

| Path | Crate | Role |
|---|---|---|
| `bin/cli` | `kailua-cli` | All-in-one binary: every service and utility subcommand |
| `build/risczero` | `kailua-build` | Embedded FPVM guest ELFs + their RISC Zero image IDs |
| `crates/kona` | `kailua-kona` | Core proving logic shared by host and zkVM guest |
| `crates/hokulea` | `kailua-hokulea` | EigenDA guest-side DA adapter |
| `crates/hana` | `kailua-hana` | Celestia guest-side DA adapter |
| `crates/contracts` | `kailua-contracts` | Solidity contracts (embedded foundry project) + alloy bindings |
| `crates/sync` | `kailua-sync` | On-chain tournament sync agent, providers, signers, transactions |
| `crates/proposer` | `kailua-proposer` | Proposal publication and resolution service |
| `crates/validator` | `kailua-validator` | Proposal validation and proof publication service |
| `crates/prover` | `kailua-prover` | Proof computation: native/Bonsai/Boundless backends, witness generation |
| `crates/rpc` | `kailua-rpc` | Withdrawal-assisting JSON-RPC server |

Other directories: `devnet/` + `kurtosis*.yaml` (local Kurtosis devnet), `scripts/` (devnet
lifecycle, license check, vendoring helpers), `testdata/` (recorded proof inputs for offline
replay), `profiling/`, `openspec/` (OpenSpec specs and in-flight change proposals).

The working tree often contains **untracked scratch** (an `optimism/` monorepo checkout,
`patched/`, `*.fake` receipts, logs). `git ls-files` is the ground truth for what belongs to the
repo. Ignore `CLAUDE.md`/`AGENTS.md` files inside `optimism/` or `patched/` — they document
upstream checkouts, not this repo.

## Commands

Always prefer the `justfile` recipes — they encode the correct feature flags:

- `just build` — release CLI, all DA providers (`-F prove -F disable-dev-mode -F eigen -F celestia`)
- `just clippy` — **the CI lint**: host workspace (default + full-feature, `--all-targets -D warnings`) plus all three guest workspaces. `RISC0_SKIP_BUILD=true` avoids guest rebuilds.
- `just test` — `RISC0_DEV_MODE=1 cargo test -F devnet`
- `just fmt` — cargo fmt for all four workspaces + `forge fmt --root crates/contracts/foundry`
- `just license-check` — every first-party source file must carry the Apache-2.0 header (exemptions are listed inside `scripts/check-license-headers.sh`)
- `just coverage` — kailua-kona coverage on a **deliberately pinned nightly** (newer LLVM llvm-cov segfaults; see the comment in ci.yml before changing)
- Devnet lifecycle: `just devnet-up` / `devnet-build` / `devnet-upgrade` / `devnet-propose` / `devnet-validate` / `devnet-fault` / `devnet-prove` / `devnet-down`

Feature flags on `kailua-cli`: `prove` (local proving), `devnet` (test-only helpers), `eigen`,
`celestia`, `experimental` (alternative guest execution path), `rebuild-fpvm` (rebuild guests
from source instead of using the committed ELFs), `disable-dev-mode`, `cuda`/`metal`.

## Soundness-critical invariants

- **FPVM image IDs** (`build/risczero/src/fpvm*.rs`) commit to the exact guest binaries the
  on-chain verifier accepts. Any change reachable from `crates/{kona,hokulea,hana}` or the guest
  workspaces changes the IDs and requires a reproducible rebake — see `build/risczero/CLAUDE.md`.
  For this reason `crates/kona`, `crates/hokulea`, and `crates/hana` pin their **own** `version`
  fields instead of `version.workspace = true`: a workspace version bump must not alter guest builds.
- **`config_hash`** (`crates/kona/src/config.rs`) must cover every consensus-relevant field of the
  rollup and L1 chain configs. `L1ChainConfig` is constructed via `::default()`, so the compiler
  will NOT flag fields added by an alloy/kona upgrade — re-audit field coverage manually on every
  bump. A missed field lets proofs be generated against a config the contracts didn't commit to.
- Contracts and guest code are the audited surface (`audits/`); behavioral changes there need
  extra scrutiny and usually a new audit.

## Dependency upgrades

Follow `book/src/dependency-upgrades.md`; the repo-specific gotchas:

1. kona is pinned by git tag (`kona-client/vX.Y.Z`) on the optimism **monorepo** (kona sources
   live under `rust/` there — check out that tag to grep upstream definitions). hokulea is pinned
   by tag, hana by rev. The `[patch.crates-io]` block at the bottom of the root `Cargo.toml` must
   track the same kona tag, or reth/alloy-evm pull in duplicate `op-alloy-*` crates.
2. `alloy*` is **exact-pinned** (`=2.0.5` style). The guest workspaces resolve their own
   lockfiles and will caret-drift to newer alloy (with breaking API changes) if the pin loosens.
3. All **four lockfiles** must move together: root plus `build/risczero/{kona,hokulea,hana}/Cargo.lock`.
4. After any guest-reachable change: `just build-fpvm` and `just build-fpvm-experimental`
   (reproducible Docker builds; runs `just vendor` first), then `just export-fpvm` and hand-paste
   the printed image IDs into `build/risczero/src/fpvm.rs` / `fpvm-experimental.rs`.
5. `cargo vendor` cannot vendor `blst` (it references sources outside its package); vendoring and
   kona-hardforks' NUT-bundle staging are both handled by `just vendor` — don't vendor by hand.
6. Re-verify: all five `just clippy` targets, `just test`, `just coverage`, and the `config_hash`
   field audit above. zkVM-only code (`crates/kona/src/r0vm_crypto.rs`) is invisible to host
   builds — confirm it compiles via the guest build, not the host lint.

## Deploying Kailua to a running chain

- `kailua-cli config --eth-rpc-url … --op-geth-url … --op-node-url …` inspects the target chain
  and prints every deployment parameter: recomputed image IDs, rollup config hash, the known
  RISC Zero verifier address for that L1, and the chain's DisputeGameFactory/Portal addresses.
- `kailua-cli fast-track` performs the full deployment in one run (verifier, treasury and game
  implementations, installation through the owner Safe, guardian `respectedGameType` switch).
  Parameter meanings: `book/src/parameters.md`; manual step-by-step path: `book/src/setup.md`
  and `book/src/upgrade.md`.
- Rehearse against the local devnet first: `just devnet-up && just devnet-build && just devnet-upgrade`.

## Conventions

- Every crate enforces rustdoc coverage via `#![cfg_attr(not(test), warn(missing_docs))]` —
  document all new public items, including enum-variant fields and clap-flattened fields.
- New source files need the Apache-2.0 license header (`just license-check`).
- CI (`.github/workflows/ci.yml`) mirrors `just clippy`/`just test`/`just coverage`, runs foundry
  tests, and additionally tests with `-F rebuild-fpvm`. Note `forge fmt` in CI does **not** cover
  `crates/contracts/foundry/scripts/` — format it deliberately.
- RPC reads in host services go through `.stall_with_context()` (infinite retry) or the
  `retry_res_ctx_timeout!` macro; every async hop is wrapped in OpenTelemetry via `await_tel!`.

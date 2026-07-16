# Contributing to Kailua

Thank you for your interest in improving Kailua!
This document describes the development workflow.
For an overview of the repository layout, see the [Project](https://boundless-xyz.github.io/kailua/project.html)
chapter of the book; the [Sequencing](https://boundless-xyz.github.io/kailua/sequencing.html) and
[Validating](https://boundless-xyz.github.io/kailua/validating.html) chapters specify the protocol that the contracts
and agents implement.

## Toolchain

* **Rust**: `rust-toolchain.toml` pins the toolchain (rustup installs it automatically). The minimum supported
  version is declared as `rust-version` in the workspace `Cargo.toml`.
* **RISC Zero**: install [`rzup`](https://dev.risczero.com/api/zkvm/install) and run `rzup install` to get the zkVM
  toolchain used for guest execution and proving.
* **just**: all common workflows are recipes in the `justfile`; run `just` to list them.
* **Docker**: required for the local devnet and for reproducible FPVM guest builds.
* **Kurtosis**: required for the local devnet.
* **svm & Foundry**: required to build and test the Solidity contracts.

## Building

`just build` compiles a release build of the `kailua-cli` binary with all data availability providers.
The workspace's cargo features:

* `prove` — enables local proof generation.
* `devnet` — enables devnet-only helpers (used by the `devnet-*` recipes and tests).
* `disable-dev-mode` — hard-disables `RISC0_DEV_MODE` fake proofs; set on all release builds.
* `eigen` / `celestia` — EigenDA (hokulea) / Celestia (hana) data availability support.
* `experimental` — accelerated in-guest cryptography (`r0vm_crypto`) and partial per-transaction execution proofs.
* `rebuild-fpvm` — rebuilds the zkVM guest binaries instead of using the committed ones; combined with
  `RISC0_USE_DOCKER=1` (the `build-fpvm*` recipes) this reproduces the committed binaries bit-for-bit.

## Testing

* `just test` — Rust test suites with dev-mode proofs (`RISC0_DEV_MODE=1 cargo test -F devnet`).
* `cargo test -p kailua-kona -F experimental` — additionally exercises the experimental code paths. This suite is
  much slower than the default one; a long-quiet run is grinding, not stuck.
* `forge test --root crates/contracts/foundry` — Solidity contract tests.
* `just coverage` — `kailua-kona` coverage via `cargo-llvm-cov`. The nightly toolchain it uses is pinned
  deliberately; see the comments in the `justfile` and `.github/workflows/ci.yml` before changing it.
* `just test-offline` — replays a recorded OP Sepolia proof from a local `./testdata` cache.

## Formatting and Linting

`just fmt` and `just clippy` must pass before submitting a pull request.
Together they cover the host workspace, all three guest workspaces, and the Solidity contracts, with warnings denied.

## Local Devnet

`just devnet-build`, `just devnet-up`, and the other `devnet-*` recipes run a full Kailua deployment against a local
Kurtosis devnet.
See the [Testing Tools](https://boundless-xyz.github.io/kailua/testing.html) chapter of the book and
`scripts/README.md` for details.

## Guest Programs (FPVM)

Changes to code that compiles into the zkVM guests — `crates/kona`, `crates/hana`, `crates/hokulea`, and
`build/risczero` — change the guest image IDs that the on-chain verifier trusts.
Read the [Dependency Upgrades](https://boundless-xyz.github.io/kailua/dependency-upgrades.html) chapter before
touching them: it documents the reproducible rebuild procedure, the image ID export flow, and the versioning policy
that keeps the `kailua-kona`, `kailua-hana`, and `kailua-hokulea` crate versions decoupled from the workspace version
to avoid unnecessary image ID churn.

## Documentation

The book under `book/` is built with `mdbook` (plus the `mdbook-admonish` and `mdbook-mermaid` preprocessors); run
`mdbook build` from `book/` and keep it green.
Operator-facing flags and commands belong in the relevant book chapter; see `book/src/SUMMARY.md` for the structure.

## Continuous Integration

Every pull request runs (`.github/workflows/ci.yml`):

* `cargo fmt` and `cargo clippy` over the host and all guest workspaces,
* the Rust test suite, including an end-to-end devnet run via Kurtosis (plus an EigenDA variant),
* `kailua-kona` coverage via `cargo-llvm-cov` on the pinned nightly,
* `forge fmt`, `forge test`, and `forge coverage` for the contracts.

Releases are built by `.github/workflows/release.yml` (binaries) and `docker-release.yml` (Docker images).

## Pull Requests

* Branch from `main` and keep pull requests focused.
* Follow the conventional-commit style used in the history (`feat: ...`, `fix: ...`, `chore: ...`).
* CI must be green; new functionality should come with tests and, where operator-facing, book updates.

<img width="100%" src="https://github.com/user-attachments/assets/c71a1018-d970-49c0-af37-ba8a66d00bea" />

# Kailua
Unlock faster finality and stronger security for your OP chain.

> [!NOTE]
> Documentation: https://boundless-xyz.github.io/kailua/


## Intro

OP Kailua lets OP chains upgrade to ZK with minimal friction, unlocking faster finality and enhanced security. With two configurable modes, rollups can adopt ZK at their own pace while meeting the proving requirements for stage 2 decentralization.

* Hybrid Mode → Uses ZK Fraud Proofs for faster finality and lower operational costs.
* Validity Mode → Uses ZK Validity Proofs for 1-Hour finality and maximum security.

## Why OP Kailua?
✅ **Pick Your Security Model**: Start with Hybrid Mode, upgrade to Validity Mode when ready. \
✅ **Fast & Easy Integration**: Works with Optimism’s Bedrock contracts (v1.4.0+).\
✅ **Stage 2 Ready**: Future-proof your rollup and accelerate your track to Stage 2 Decentralization

## How does OP Kailua Work?
**Hybrid Mode**: ZK Fraud Proofs for Faster Dispute Resolution

OP Kailua Hybrid Mode replaces traditional fault-proof mechanisms with ZK Fraud proofs, improving security without requiring rollups to immediately switch to full validity proofs.

- Uses the RISC-Zero zkVM to verifiably run Optimism's [Kona][kona] and secure rollups with cryptographic proofs enabling faster finality and reduced operational costs.
- Designed to require constant collateral lockups from both proposers and validators (challengers), whereas the Bisection-based fault dispute game backed by Cannon requires a linear number of deposits proportional to the number of proposals/challenges.
- The fault proofs are estimated to require on the order of 100 billion cycles to prove in the worst case, which, on Bonsai, would cost on the order of 100 USD and take around an hour to prove.
- All proving costs are borne by the dishonest party in the protocol, whether that is the proposer or validator.


**Validity Mode**: ZK Validity Proofs for 1-Hour Finality

Validity Mode turns OP chains into a ZK Rollup, eliminating disputes entirely since every transaction is validity proven.

- Unlocks 1-hour finality
- No challenges, no disputes—ZK Validity Proofs replace interactive fault proofs.

## Audits
Kailua has undergone the following audits throughout its development:
* [2025 FEB 18](audits/veridise-250218.pdf)
* [2025 MAY 22](audits/veridise-250522.pdf)
* [2025 JUN 16](audits/veridise-250616.pdf)
* [2025 OCT 23](audits/veridise-251023.pdf)
* [2026 FEB 12](audits/veridise-260212.pdf)

## Prerequisites
1. [rust](https://www.rust-lang.org/tools/install) — rustup installs the pinned toolchain from `rust-toolchain.toml` automatically.
2. [just](https://just.systems/man/en/) — runs all development workflows; `just` lists the available recipes.
3. [docker](https://www.docker.com/) — required for the local devnet and reproducible FPVM guest builds.
4. [kurtosis](https://docs.kurtosis.com/install) — required for the local devnet only.
5. [svm](https://github.com/alloy-rs/svm-rs) — installs the solc version used by the contracts.
6. [foundry](https://book.getfoundry.sh/getting-started/installation) — builds and tests the contracts.

## Proving Demo

You can test out Kailua's validity proving on a running chain through the following commands:

1. `just build`
    * Compiles a release build of Kailua
2. `just demo [BLOCKS_PER_PROOF] [L1_RPC] [BEACON_RPC] [L2_RPC] [OP_NODE_RPC]:`
    * Runs the release build against the target chain endpoints.
    * See [here](https://boundless-xyz.github.io/kailua/validator.html#delegated-proof-generation) for advanced proving configuration

## Local Devnet

You can deploy a local optimism devnet equipped with Kailua through the following commands:

1. `just devnet-fetch`
    * Fetches `v1.16.7` of the `optimism` monorepo.
2. `just devnet-build`
    * Builds the local Kailua binaries.
    * The OP Stack services themselves use prebuilt artifacts from the Optimism release pipeline.
3. `just devnet-up`
    * Starts a local OP Stack devnet using Kurtosis.
    * Writes the devnet descriptor to `devnet/kurtosis-devnet.json`.
    * Dumps the deployment output into `devnet.log` for inspection.
4. `just devnet-upgrade`
    * Upgrades the devnet to use the `KailuaGame` contract.
    * Auto-discovers RPC endpoints and default keys from `devnet/kurtosis-devnet.json`, but can still take explicit overrides.
5. `just devnet-propose`
    * Launches the Kailua proposer.
    * This runs the sequences, which periodically creates new `KailuaGame` instances.
6. `just devnet-validate`
    * Launches the Kailua validator.
    * This monitors `KailuaGame` instances for disputes and creates proofs to resolve them.
    * (VALIDITY PROVING) Use `just devnet-validate [block-height]` to generate validity proofs to fast-forward finality until the specified L2 block height.
    * (DEVELOPMENT MODE): Use `RISC0_DEV_MODE=1` to use fake proofs.
7. `just devnet-rpc`
    * Launches the Kailua RPC.
    * This provides utility RPC methods for initiating withdrawals.
    * Listens on http://127.0.0.1:1337 and ws://127.0.0.1:1337 by default.
8. `just devnet-fault`
    * Deploys a single `KailuaGame` instance with a faulty sequencing proposal.
    * Tests the validator's fault proving functionality.
    * Tests the proposer's canonical chain tracking functionality.
9. After you're done:
    * `just devnet-down` to remove the running Kurtosis enclave.
    * `just devnet-clean` to remove the local descriptor and logs.

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for the full development workflow, and the
[Project](https://boundless-xyz.github.io/kailua/project.html) chapter of the book for a map of the crates in this
repository. The most common commands:

* `just build` — compile a release build of the Kailua CLI.
* `just test` — run the Rust test suites using dev-mode proofs.
* `just fmt` — format all Rust workspaces and the Solidity contracts.
* `just clippy` — lint all Rust workspaces.

Security vulnerabilities should be reported as described in [SECURITY.md](SECURITY.md).

## Questions, Feedback, and Collaborations

We'd love to hear from you on [Discord][discord] or [Twitter][twitter].

[discord]: https://discord.gg/risczero
[twitter]: https://twitter.com/risczero
[kona]: https://github.com/op-rs/kona

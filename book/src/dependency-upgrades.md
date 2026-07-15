# Dependency Upgrades

This chapter is for Kailua maintainers (and forks) who need to upgrade Kailua to a newer release of
[Kona](https://github.com/ethereum-optimism/optimism), [Hokulea](https://github.com/Layr-Labs/hokulea), or
[Hana](https://github.com/celestiaorg/hana), along with the `alloy`/`op-alloy`/`revm`/`reth` dependency tree that
rides along with them.

Upgrading these dependencies is unlike upgrading dependencies in a normal Rust project, because most of the affected
code is compiled into RISC Zero zkVM guest programs whose image IDs are consensus-critical:

* A successful upgrade **always changes the FPVM image IDs**, which requires an [on-chain upgrade](fpvm-upgrade.md)
  before the new binaries can be used in production.
* A careless upgrade can introduce a **soundness gap** (e.g. an unhashed configuration field) that no compiler error
  or test failure will surface unless you follow the audit steps below.

```admonish warning
Do not treat a green build as a finished upgrade.
The soundness re-audit in [Step 3](#step-3-re-audit-the-config-hash) is mandatory on every upgrade.
```

## How the Pieces Fit Together

Recall from the [project structure](project.md) that each supported DA option has a wrapper crate and a corresponding
zkVM guest workspace:

| Guest program | Guest workspace | Compiled-in crates |
|---|---|---|
| `kailua-fpvm-kona` | `build/risczero/kona` | `kailua-kona` |
| `kailua-fpvm-hokulea` | `build/risczero/hokulea` | `kailua-kona` + `kailua-hokulea` |
| `kailua-fpvm-hana` | `build/risczero/hana` | `kailua-kona` + `kailua-hana` |

Three properties drive the entire upgrade procedure:

1. **Each guest workspace is an independent Cargo workspace** with its own `Cargo.toml`, `Cargo.lock`, and `[patch]`
   section. Version and tag changes must be applied to each of them, not just the root workspace.
2. **Guest builds are reproducible Docker builds** (`RISC0_USE_DOCKER=1`, `--locked`) over *vendored* sources.
   Reproducibility comes from the pinned RISC Zero Docker toolchain image plus the committed lockfiles — not from
   your local build cache.
3. **Crate versions feed into image IDs.** Cargo passes each crate's version into symbol mangling (`-C metadata`),
   so bumping the version of any crate that compiles into a guest changes that guest's image ID — even with zero
   source changes. This is why `kailua-kona`, `kailua-hokulea`, and `kailua-hana` deliberately do **not** inherit
   the workspace version: it lets you bump the workspace (host) version without churning guest image IDs, and bump
   an individual wrapper crate to churn only the guests that depend on it.

## Standard vs. Experimental Builds

Every guest program additionally exists in two variants, selected by the `experimental` cargo feature:

* The **standard** build already supports proof decomposition down to the block level: derivation-only proofs,
  execution-only proofs, and the recursive stitching that combines such proofs into a complete proposal proof.
* The **experimental** build extends decomposition *below* the block level, allowing a single block's execution to
  be proven through partial (per-transaction-chunk) execution traces. Inside the zkVM it also swaps the EVM's
  precompile cryptography for RISC Zero's accelerated implementations (the `r0vm_crypto` module, backed by the
  `risc0-crypto-evm` dependency).

Mechanically this is one codebase, not two: `bin/cli`'s `experimental` feature fans out to `kailua-kona`,
`kailua-prover`, and `kailua-validator`, and `build/risczero/build.rs` forwards the flag into the guest builds. The
same source tree therefore produces two program families — six committed guest binaries in total, with two sets of
image ID constants (`build/risczero/src/fpvm.rs` and `build/risczero/src/fpvm-experimental.rs`) and two
configuration blocks in [Setup](setup.md).

This split has consequences for every step of an upgrade:

* **Everything in this chapter happens twice.** The clippy/test matrix, the ELF bakes, the image ID constant
  updates, the documentation values, and (where deployed) the on-chain rollout each have a standard and an
  experimental leg. Skipping the experimental leg does not produce a stale-but-working experimental build — it
  produces one that no longer matches the host code.
* **A given `kailua-cli` binary embeds exactly one family, chosen at compile time.** The `experimental` feature
  adds fields to the witness types (`pe_witness`, `partial_executions` in `crates/kona/src/witness.rs`), so the
  serialized witness format itself differs between the two builds. Host and guest must be built with the same
  feature — a standard host cannot drive an experimental guest or vice versa, and there is no runtime toggle.
* **Some experimental code compiles only inside the guest bake.** `r0vm_crypto` is gated on `experimental` *and*
  `target_os = "zkvm"`, so no host build, clippy invocation, or test ever compiles it — the experimental FPVM build
  is its only compile check, and an error in it surfaces half an hour into the Docker bake. Since it reimplements
  revm's `Crypto` trait, diff it against the vendored `revm-precompile` interface on every `revm` bump *before*
  baking.
* **The experimental EVM wrappers track upstream trait surfaces.** `crates/kona/src/evm/` wraps the OP EVM in
  adapters implementing upstream hook traits (e.g. `alloy-op-evm`'s post-execution hooks). When an upgrade adds
  methods to those traits, the wrappers gain mandatory delegations. New hooks are usually inert under Kailua's
  single-pass execution, but verify that for each one and delegate to the inner EVM rather than stubbing a default.

## Step 1: Align Versions

The source of truth for the entire dependency tree is the lockfile of the Kona release you are upgrading to:
`rust/Cargo.lock` in the [optimism monorepo](https://github.com/ethereum-optimism/optimism) at the new
`kona-client/vX.Y.Z` tag. Kailua's workspace dependencies must resolve to the same versions Kona itself was built
and tested against.

Update the pinned tags and revisions in **all** of the following files:

* `Cargo.toml` (root): the `kona-*`, `hokulea-*`/`canoe-*`/`eigenda-*`, and `hana-*` dependency blocks, **and** the
  `[patch.crates-io]` section (the `op-alloy-*`, `alloy-op-evm`, and `op-revm` patches must point at the same tag).
* `build/risczero/kona/Cargo.toml`, `build/risczero/hokulea/Cargo.toml`, `build/risczero/hana/Cargo.toml`: each
  guest manifest repeats the relevant `[patch]` entries. These also pin the RISC Zero **accelerated forks** of
  `blst` and `c-kzg` — when the new dependency tree raises its `c-kzg`/`blst` requirements, matching fork releases
  must exist and be bumped here too.
* `build/risczero/.cargo/config.toml`: the `[source."git+..."]` replacement blocks name the tags/revisions
  explicitly and must match, or the vendored Docker build will fetch nothing.

Then align the shared crates (`alloy-*`, `op-alloy-*`, `op-revm`, `revm`, `alloy-evm`, `alloy-op-evm`, ...) in the
root `[workspace.dependencies]` to the versions in Kona's lockfile. Two traps deserve special attention:

```admonish danger title="The caret-drift trap"
Pin the `alloy` 2.x family with **exact** version requirements (`=2.0.5`), not caret ranges.
The host workspace depends on the `alloy` meta-crate, which exact-pins its whole family — so a host-only build
will look fine either way. But the guest workspaces reference the individual `alloy-*` crates directly, and a caret
range there floats up to whatever newer minor release exists on crates.io, which Kona predates. This failure mode
only appears inside the FPVM build, long after the host compiles.
After updating, verify the guest lockfiles (e.g. `build/risczero/kona/Cargo.lock`) resolve to the exact intended
versions.
```

```admonish danger title="The source-split trap"
Kona and `alloy-op-evm` consume some crates (notably `op-revm`) from *inside* the optimism monorepo via path
dependencies. The crates.io release with the same name and version is a **different crate** to Cargo, so types like
`OpSpecId` will not unify across the two copies and you will get baffling trait-bound errors. The fix is the
`[patch.crates-io]` entry pointing that crate at the optimism git tag — in the root manifest **and** in each of the
three guest manifests.
```

Exact pins cut both ways: a **stale** exact pin left over from the previous upgrade forces Cargo to keep the old
version alongside the new one, and two coexisting copies of the same crate produce baffling "expected `Evm`, found
`Evm`" trait-bound errors. Every exact pin must be revisited on every upgrade.

Finally, expect release lag. Kona typically releases first, and Hokulea, Hana, and host-only dependencies that track
alloy (e.g. `risc0-steel`, `boundless-market`) may not yet have cut a release against the new tree. It is normal to
temporarily pin these to interim git revisions and swap to proper tags in a follow-up once upstream catches up —
the v1.5.2 upgrade shipped exactly that way.

## Step 2: Migrate the Source

With versions aligned, work through the compile errors host-first, since host iteration is much faster:

```shell
RISC0_SKIP_BUILD=true cargo clippy --bin kailua-cli --locked --all-targets -- -D warnings
```

`RISC0_SKIP_BUILD=true` skips the guest builds so you can iterate on `crates/kona`, `crates/hokulea`, `crates/hana`,
and the host crates. Repeat with the feature combinations used by `just clippy` (including
`-F devnet -F eigen -F celestia -F experimental`), then check each guest workspace via its own manifest path.

A few hard-won rules for this phase:

* **Grep the right checkout.** When verifying how an upstream struct changed, resolve the tag to its commit hash via
  `Cargo.lock` and read that exact checkout under `~/.cargo/git/checkouts/` (or the vendored copy under
  `build/risczero/vendor/` once vendored). Similarly-named checkouts of forks can contain phantom fields.
* **The compiler, not grep, is ground truth for field existence.** Upstream fields can be `#[cfg(feature = ...)]`-gated
  behind features Kailua does not enable (e.g. `RollupConfig::fjord_max_sequencer_drift`), so a field visible in
  source may be absent from the compiled struct.
* **`rkyv` mirrors must round-trip new fields, not default them.** `crates/kona/src/rkyv/` mirrors upstream types
  for witness serialization. When an upstream type gains a field, add it to the mirror and its round-trip test.
  Note that `rkyv` only implements its traits for tuples up to arity 13 — nest a sub-tuple when a mirror outgrows
  that, following the existing pattern.
* **Anything serialized into the witness must also be bound by the precondition hash.** When a mirrored type gains
  a field, check the corresponding `flatten_*` helpers: a field that is rkyv-encoded but not hashed is
  attacker-controlled.
* **Audit new host-side configuration fields, not just removed ones.** When an upstream struct Kailua constructs
  (e.g. Kona's `SingleChainHost`) gains a field, filling it with `Default::default()` compiles but silently adopts
  whatever new behavior upstream chose. In the v1.5.2 upgrade, Kona's new `data_format` field defaulted to a new
  directory-based preimage store format while Kailua's own stores remained RocksDB — preimages seeded through one
  would have been invisible to the other if this were not examined. Review what each new field does and choose its
  value deliberately.

## Step 3: Re-audit the Config Hash

`kailua_kona::config::config_hash` (`crates/kona/src/config.rs`) commits to the rollup and L1 chain configuration a
fault proof is about. **Every field of every hashed struct must be included in the hash** — an omitted field means
two semantically different configurations collide to the same on-chain commitment, letting a prover prove under a
configuration the verifier never pinned.

On every upgrade, re-audit field coverage of all hashed structs: `RollupConfig`, `SystemConfig`, `HardForkConfig`,
`AltDAConfig`, `BaseFeeConfig`, `ChainGenesis`, `BlobParams`, and `L1ChainConfig`.

Most of these are compiler-protected: `test_config_hash` constructs them with exhaustive struct literals (no
`..Default::default()`), so a new upstream field breaks compilation until it is named — and hopefully hashed.

```admonish danger title="L1ChainConfig is not compiler-protected"
`L1ChainConfig` is an alias for `alloy_genesis::ChainConfig` and is constructed via `::default()` in the tests, so
a new field added upstream compiles silently and silently escapes `l1_config_hash`.  On every alloy bump, diff the full
field list of `ChainConfig` in the vendored sources against the fields consumed by `l1_config_hash`, one by one.
```

Audit against the *vendored* sources (`build/risczero/vendor/`) — that is exactly the code the guest compiles.
Remember that adding any field to the hash changes the hash value for **all** configurations (even those where the
new field is `None`), which changes the on-chain `ROLLUP_CONFIG_HASH` alongside the image IDs.

## Step 4: Vendor the Guest Dependencies

The Docker guest builds compile from vendored sources. Refresh them with:

```shell
just vendor
```

This runs `cargo vendor` across the three guest workspaces into `build/risczero/vendor` and then stages the out-of-crate
files that `cargo vendor` omits (e.g. the NUT bundle JSONs read by `kona-hardforks`' build script — see
`scripts/stage-nut-bundles.sh`).

```admonish warning title="Do not over-vendor"
`cargo vendor` prints a complete `[source]` replacement list at the end of its run. Do **not** paste it wholesale
into `build/risczero/.cargo/config.toml`. Crates that compile C sources living *outside* their own crate directory
(e.g. the RISC Zero `blst` fork builds `../../src/server.c`) cannot be vendored — `cargo vendor` never copies the
out-of-crate C files, and the vendored build fails with "No such file or directory". Keep the replacement list
minimal (the kona/hokulea/hana/reth trees and pure-Rust git dependencies) and let the C-source crates remain
git-fetched inside Docker.
```

## Step 5: Bump Versions and Lockfiles

Decide which crate versions to bump based on which guests actually changed:

* Bump the root `[workspace.package]` version — all host-side crates inherit it, and it never affects image IDs.
* Bump `crates/kona` only if the Kona-level code changed (this churns **all three** guest image IDs).
* Bump `crates/hokulea` / `crates/hana` for changes scoped to those wrappers (churns only that guest's image ID).
* Mirror any bumped guest-member versions into the corresponding guest `Cargo.toml` **and** `Cargo.lock` files —
  the Docker build runs with `--locked` and will refuse a stale lockfile.

Finally, refresh the root lockfile with `cargo update -w` (or targeted `cargo update -p` invocations) and re-run the
host checks.

## Step 6: Rebake the FPVM Binaries

The reproducible ELFs are built inside Docker and committed to the repository together with their image IDs.

A rebake is required not only for Kona/Hokulea/Hana changes: bumping the RISC Zero toolchain or the `risc0-*`
crates also changes every image ID, and a zkVM version bump can additionally change the `CONTROL_ROOT`/`CONTROL_ID`
and the required on-chain verifier — check the [RISC Zero verifier documentation](https://dev.risczero.com/api/blockchain-integration/contracts/verifier)
when the `RISC0_VERSION` reported by `kailua-cli config` changes.

```admonish tip
Each variant's bake leaves tens of gigabytes of buildkit cache in the Docker VM, and a full upgrade bakes at least
two variants back-to-back. Check `docker system df` first and reclaim space with `docker builder prune -f` if
needed — a full VM disk surfaces as an opaque `risc0-build` panic ("docker build failed") with the real
"no space left on device" error buried in the log. Losing the cache only costs time; reproducibility comes from the
pinned toolchain image and `--locked`, not the cache.
```

For the standard build:

```shell
just build-fpvm     # reproducible Docker build of all three guests (~30 min)
just export-fpvm    # writes build/risczero/src/bin/*.bin and prints each image ID
```

Then manually paste the printed `[u32; 8]` image IDs into the constants in `build/risczero/src/fpvm.rs`.

Repeat for the experimental build: `just build-fpvm-experimental`, `just export-fpvm` (the experimental CLI writes
the `*-experimental.bin` files), and update `build/risczero/src/fpvm-experimental.rs`.

```admonish warning title="export exports whatever CLI you last built"
`just export-fpvm` runs `target/release/kailua-cli export`, which writes out the guest binaries **embedded in that
CLI binary** — the variant is decided by how the CLI was built, not by any export flag. Interleave strictly per
variant: bake standard → export → paste IDs → bake experimental → export → paste IDs. If a bake fails, the
previously built CLI remains in `target/release`, and running export anyway will silently re-export the *other*
variant's binaries without any error.
```

Two verification habits are worth keeping:

* **Byte-identical binaries are your regression check.** If a guest was not supposed to change (e.g. you bumped only
  `crates/hokulea`), its rebaked `.bin` must be byte-identical to the committed one — `git status` simply won't list
  it. If an "untouched" guest's binary changed, something leaked into it; find out what before proceeding.
* **The ID constants and the `.bin` files must come from the same bake.** `kailua-cli config` asserts at runtime that
  the computed image ID of each embedded ELF matches its hardcoded constant, so a mismatched paste fails loudly —
  but only once someone runs it.

Finally, update the example image IDs in [Setup](setup.md). Note the format difference: `fpvm.rs` stores each ID as
`[u32; 8]` words, while the `config` command output shown in the book displays the RISC Zero digest form — each word
serialized as little-endian bytes and concatenated. The `just export-fpvm` log prints both representations' inputs;
the easiest path is to run `kailua-cli config` against a live rollup and copy its output.

## Step 7: Test

Run the full local suite:

```shell
just fmt
just clippy   # host (default + full-featured) and all three guest workspaces, -D warnings
just test     # cargo tests with RISC0_DEV_MODE=1
```

Run the `kailua-kona` library tests both with and without `-F experimental` — the partial-execution and EVM-wrapper
code paths described in [Standard vs. Experimental Builds](#standard-vs-experimental-builds) are only compiled and
exercised under the flag (except `r0vm_crypto`, which only the experimental bake itself compiles). Be patient: the
flag also enables additional stitching tests that iterate over pairs of boot configurations against the ~290 MiB
`testdata` fixture, so the suite runs many times longer than the standard one — a long-quiet test process is
grinding, not deadlocked. For the same reason, `just coverage` deliberately omits the flag; the experimental suite runs in the
regular CI test jobs instead.

The recorded-block replay tests in `crates/kona` re-derive and re-execute real blocks against a committed preimage
fixture (`crates/kona/testdata`). A Kona upgrade can change *which* preimages the pipeline requests, failing these
tests with missing-key errors even though the code is correct; in that case the fixture needs to be regenerated by
re-running the recorded proofs in dev mode (`RISC0_DEV_MODE=1 kailua-cli prove --native --data-dir
./crates/kona/testdata ...` with the boot parameters pinned by the tests), not the code reverted. When regenerating,
flush RocksDB's write-ahead log into the `.sst` files before committing — preimages that only exist in the trailing
`*.log` WAL file open fine locally but are matched by `.gitignore`'s `*.log` rule, producing a fixture that passes
on your machine and fails in CI.

```admonish tip title="Missing preimages hang, they don't error"
Kona's host backend retries a failing or unresolvable preimage hint indefinitely.
Whether in the replay tests, a devnet proving run, or production, a preimage that was seeded into the wrong store
(or never seeded) presents as a *silent infinite hang* at the affected proving phase — not as an error.
If a proving pipeline stalls after an upgrade, suspect a missing or misrouted preimage before anything else.
```

Then validate end-to-end on the local devnet:

```shell
just devnet-fetch
just devnet-build-fpvm      # debug build with locally-rebuilt guests
just devnet-up
just devnet-upgrade         # deploy contracts against the new image IDs
just devnet-propose         # run a proposer...
just devnet-validate        # ...and a validator
just devnet-fault 1 [PARENT] # publish a faulty proposal and watch it get disproven
```

A faulty proposal being successfully disproven, and honest proposals resolving, exercises the full pipeline: new
guests, new config hash, and new contracts working together.

```admonish note
`just devnet-build-fpvm*` compiles the guests **locally** (not in Docker), so the resulting image IDs will not match
the committed reproducible ones — that is expected, and the devnet deploys against whatever it built. Only the
Docker bake from Step 6 produces the canonical IDs.
```

## Step 8: Roll Out On-chain

For an already-migrated rollup, the new image IDs (and, if the config hash computation changed, the new
`ROLLUP_CONFIG_HASH`) must be pushed on-chain by upgrading the `KailuaVerifier` implementation as described in
[FPVM Upgrade](fpvm-upgrade.md). Fresh migrations pick the new values up automatically through the standard
[on-chain migration](upgrade.md) flow.

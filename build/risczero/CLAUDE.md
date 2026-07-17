# kailua-build — FPVM guests and image IDs

This crate embeds the three FPVM guest programs (kona = Ethereum-blob DA, hokulea = EigenDA,
hana = Celestia) and exports each as `KAILUA_FPVM_<NAME>_{ELF,PATH,ID}`. The image IDs here are
**soundness-critical**: the on-chain verifier only accepts receipts bound to these exact IDs.
Treat any diff to an ID array or a committed `.bin` as a consensus change.

## The three-way constant switch

`src/lib.rs` selects exactly one source for the same constant names:

- `rebuild-fpvm` feature → `include!`s risc0-build's generated `methods.rs` from `OUT_DIR`
  (`build.rs` builds the guest workspaces, in Docker when `RISC0_USE_DOCKER=1`).
- default → `src/fpvm.rs` (committed ELFs in `src/bin/*.bin` + hardcoded IDs).
- `experimental` → `src/fpvm-experimental.rs` (the `-experimental.bin` variants).

`src/fpvm.rs` and `src/fpvm-experimental.rs` are **hand-maintained**, not generated:

1. `just build-fpvm` / `just build-fpvm-experimental` reproducibly rebuilds the guests in Docker
   (runs `just vendor` first) and writes fresh ELFs.
2. `just export-fpvm` writes the ELFs to `src/bin/*.bin` and prints each image ID as a Rust
   array literal.
3. A human pastes the printed IDs into the two `fpvm*.rs` files. **Preserve the existing doc
   comments** when pasting — only the array contents should change.

These two files are exempt from the license-header check (see `scripts/check-license-headers.sh`).

## Guest workspaces (`kona/`, `hokulea/`, `hana/`)

Each is an independent cargo workspace with its own `Cargo.lock` and `rust-toolchain.toml`,
path-depending on `crates/{kona,hokulea,hana}`. Consequences:

- Dependency upgrades must update these lockfiles too, and their resolution can drift from the
  host workspace (host `alloy` is exact-pinned for this reason).
- Reproducible builds require the vendored deps in `vendor/` (`just vendor`; handles the blst
  can't-vendor and kona-hardforks NUT-bundle gotchas — never run `cargo vendor` directly).
- Anything that changes the compiled guest — source, deps, or even the `version` field of
  `crates/{kona,hokulea,hana}` — changes the image IDs. Those three crates deliberately pin
  their own `version` instead of inheriting the workspace version.

## Lint quirk

The crate's `missing_docs` lint is gated as
`#![cfg_attr(not(any(test, feature = "rebuild-fpvm")), warn(missing_docs))]` because the
rebuild-fpvm path includes generated, undocumented code and CI runs `cargo test -F rebuild-fpvm`.

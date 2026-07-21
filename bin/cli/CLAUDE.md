# kailua-cli

The single shipped binary. `lib.rs` enumerates the subcommands: four launch the sibling-crate
services (`propose`, `validate`, `rpc`, and `prove` — the prover child process, which exits with
code **111** on insufficient L1 data), the rest are utilities implemented here: `config`
(deployment parameter inspection), `fast-track` (one-shot deployment), `test-fault` (devnet
fault injection), `benchmark`, `demo` (contract-free continuous validity proving), `bonsai`/
`boundless` (receipt download), and `export` (writes guest ELFs and prints the image IDs that
get hand-pasted into `build/risczero/src/fpvm*.rs`).

Run subcommands through the repo-root `justfile` recipes where one exists — they encode the
required flags and devnet endpoint resolution.

## Conventions

- This is a lib + bin pair; the `missing_docs` lint only covers the lib target (bin items have
  no external visibility), so keep subcommand logic in `src/<command>.rs` modules under the lib,
  not in `main.rs`. Doc comments on `KailuaCli` variants and clap fields double as `--help` text.
- New subcommand = new module + `KailuaCli` variant + wiring in `main.rs`, plus feature-flag
  forwarding in `Cargo.toml` if it touches gated crates.

## Integration test

`tests/devnet.rs` is the end-to-end test: it drives a full Kurtosis devnet (deploy via
fast-track, propose, inject faults, validate). CI runs it in dedicated devnet jobs; locally it
needs Kurtosis/Docker and a devnet-featured build (`just devnet-build`). With `-F experimental`
it exercises the experimental guest path too.

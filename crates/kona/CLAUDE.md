# kailua-kona — core proving logic

This crate is compiled into the FPVM guests as well as the host. **Every change here shifts the
guest image IDs** (rebake + hand-paste flow in `build/risczero/CLAUDE.md`), and much of it is
consensus logic: bugs here are soundness bugs, not crashes.

## Layout

- `boot.rs` / `journal.rs` — the guest's public I/O: `BootInfo` in, packed `ProofJournal` out.
  The journal is what the contracts verify against; its encoding is part of the protocol.
- `config.rs` — `config_hash` commits the rollup config **and** the L1 chain config into the
  journal. `L1ChainConfig` is alloy's `ChainConfig` built via `::default()`, so new upstream
  fields are silently unhashed unless added by hand — re-audit on every alloy/kona bump.
- `executor.rs` / `driver.rs` — block building and the derivation pipeline over kona, including
  the `Cached*` pipeline-stage wrappers.
- `client/` — guest entrypoints: `stateless` (single run) and `stitching` (composing multiple
  proofs, e.g. splitting a large range).
- `oracle/` — preimage oracle implementations (local, vec-backed, etc.).
- `precondition/` — blob-equivalence precondition validation for proposals.
- `rkyv/` — zero-copy witness serialization wrappers. Convention: each remote type gets a
  tuple-encoding `Rkyved*` alias plus a `rkyv::with` wrapper struct; rkyv tuples cap out around
  arity 13, so wide structs need nested tuples. Doc templates for these are uniform — copy a
  neighbor.
- `evm/` — the `experimental`-gated PostExecEvm execution path.
- `r0vm_crypto.rs` — zkVM-only accelerated crypto (`cfg`-gated to the zkvm target). Host builds,
  lints, and rustdoc **cannot see this file**; verify changes via a guest build.
- `blobs.rs` — KZG helpers, `hash_to_fe` field-element mapping for intermediate outputs.

## Testing

- `just test` runs the suite with `RISC0_DEV_MODE=1`.
- `just coverage` gates kailua-kona at zero uncovered functions (pinned nightly — see ci.yml
  comment before touching the toolchain).
- With `-F experimental`, the stitching tests grind O(n²) pairs over a ~290MB testdata copy per
  pair — slow, not deadlocked. CI runs coverage without the flag.
- `testdata/` here holds a recorded RocksDB preimage store consumed by tests; the repo-root
  `testdata/` holds full recorded proof inputs for `just test-offline` (offline OP Sepolia
  replay). To regenerate after a format change: run a dev-mode native prove against a live
  chain/devnet and capture the witness cache before the R0VM stage.

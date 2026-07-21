# Host service crates

How the pieces connect at runtime (all launched as `kailua-cli` subcommands):

1. **kailua-sync** is the shared foundation: a `SyncAgent` follows DisputeGameFactory events and
   maintains a local RocksDB view (under `--data-dir`) of the proposal tournament, alongside
   provider construction, signers (local/AWS/GCP), transaction publication (`Transact`), and
   telemetry. Proposals are identified by their DGF index; Kailua games use
   `kailua_sync::KAILUA_GAME_TYPE`.
2. **kailua-proposer** publishes sequencing proposals through the treasury (intermediate output
   roots ride along as blob sidecars; a duplication counter in `extra_data` disambiguates
   re-proposals) and resolves finalizable proposals, pruning eliminated opponents in
   `ELIMINATIONS_LIMIT`-sized batches.
3. **kailua-validator** runs two cooperating tasks joined by a `DuplexChannel<Message>`:
   `handle_proposals` classifies synced proposals and dispatches proof requests;
   `handle_proof_requests` computes proofs by spawning **child `kailua-cli prove` processes**.
   Fault proofs disprove a wrong output; validity proofs confirm a correct one; trail faults
   (non-canonical blob trailing data) are proven KZG-only via `proveTrailFault`, no ZK receipt.
4. **kailua-prover** implements the actual proving: witness generation from RPC preflight,
   then a native zkVM run, Bonsai, or the Boundless market (`risczero/` backends).
5. **kailua-rpc** serves a withdrawal-assisting JSON-RPC API on top of the sync state.

## Cross-crate contracts

- Exit code **111** from a prove invocation means "insufficient L1 data" — the caller retries
  with a newer `l1_head` rather than treating it as failure.
- Proof files are named by `proof_file_name(image_id, journal)`; a pre-existing file containing
  a valid receipt is never overwritten.
- Every chain read uses `.stall_with_context()` (retries forever) or `retry_res_ctx_timeout!`;
  wrap new async calls in the `await_tel!`/`await_tel_res!` telemetry macros like their neighbors.

## Feature flags

`devnet` gates test-only behavior (e.g. fake-receipt patching in the validator, L1-head
jump-back); `eigen`/`celestia` gate the DA backends; `experimental` selects the alternative
guest execution path. Keep flag forwarding consistent with the matrices in `bin/cli/Cargo.toml`
and lint with `just clippy` (which covers default and full-feature builds).

## Guest-reachable crates

`kona`, `hokulea`, and `hana` are compiled **into the FPVM guests**: changes there alter the
RISC Zero image IDs and require the rebake flow in `build/risczero/CLAUDE.md`. They must stay
`no_std`-compatible in guest paths and keep their independently pinned `version` fields.

Most host crates have no unit tests; end-to-end coverage comes from `bin/cli/tests/devnet.rs`
against the Kurtosis devnet, and unit-test depth is concentrated in `crates/kona`.

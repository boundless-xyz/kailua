# kailua-contracts — Solidity + bindings

A Rust crate wrapping the foundry project in `foundry/`. `build.rs` compiles the contracts (via
foundry-compilers) and the crate exposes alloy `sol!`-style bindings used by all host services —
Solidity signature changes surface as Rust compile errors in `sync`/`proposer`/`validator`.

## Contracts (`foundry/src/`)

- `KailuaTournament.sol` — base: proposal children, elimination/tournament resolution logic.
- `KailuaTreasury.sol` — proposal entry point, participation bonds, eliminations, the anchor.
- `KailuaGame.sol` — the per-proposal dispute game instance (IDisputeGame).
- `KailuaVerifier.sol` — proof verification entry, wraps the RISC Zero verifier.
- `KailuaLib.sol` — shared constants/helpers (game type, KZG verification, blob math).
- `Proxy.sol`; deployment scripts in `foundry/scripts/` (`Deploy.s.sol`, `UpgradeVerifier.s.sol`).

Protocol semantics (vanguard advantage, duplication counters, trail faults, bonds) are specified
in `book/src/{design,game,treasury,dispute}.md` — keep NatSpec consistent with the book. This is
the audited surface (`audits/`); behavior changes need commensurate review.

## Gotchas

- `forge build` does **not** compile `scripts/` by default — pass the script paths explicitly
  when checking them.
- CI's `forge fmt` check does **not** cover `foundry/scripts/`; format it yourself.
- `@inheritdoc` resolution breaks if two source units both declare `IDisputeGame` — KailuaGame's
  NatSpec relies on there being exactly one import path for it.
- Format with `forge fmt --root crates/contracts/foundry` (part of `just fmt`).

## Testing

`forge test --root crates/contracts/foundry` (CI runs this). Tests in `foundry/test/` cover
proposals, bonds, blob/claim disputes, KZG, rewards, deploys, and proxy upgrades — extend
`KailuaTest.t.sol` fixtures rather than re-deriving setup.

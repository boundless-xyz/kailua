## 1. Update build.rs to load remappings

- [x] 1.1 Add remappings.txt parsing: read `foundry/remappings.txt`, parse each non-empty line with `Remapping::from_str()`, collect into a `Vec<Remapping>`
- [x] 1.2 Apply parsed remappings to `ProjectPathsConfig` via the builder's remappings setter before building the project

## 2. Update rerun-if-changed directives

- [x] 2.1 Add `cargo:rerun-if-changed=foundry/remappings.txt` directive
- [x] 2.2 Add `cargo:rerun-if-changed=foundry/foundry.toml` directive
- [x] 2.3 Add `cargo:rerun-if-changed=foundry/lib` directive for submodule dependency changes

## 3. Verification

- [ ] 3.1 Run `cargo build -p kailua-contracts` and confirm it compiles successfully
- [x] 3.2 Run `forge build` in `crates/contracts/foundry/` and confirm it still compiles successfully
- [x] 3.3 Run `forge test` in `crates/contracts/foundry/` and confirm all tests pass

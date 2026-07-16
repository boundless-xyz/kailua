set fallback := true

devnet_name := "kailua"
devnet_enclave := "kailua-devnet"
devnet_runtime_dir := "devnet"
devnet_descriptor := "devnet/kurtosis-devnet.json"
devnet_package_dir := "devnet/optimism-package"
devnet_data_dir := "devnet/data"
devnet_log := "devnet/devnet.log"
devnet_propose_dir := "devnet/propose"
devnet_validate_dir := "devnet/validate"
devnet_optimism_commit := "3019251e80aa248e91743addd3e833190acb26f1"
devnet_package_commit := "89e0b8cacab9f7e9f74d53b72d4870092825d577"

# default recipe to display help information
default:
  @just --list

# Vendor the guest workspace dependencies for reproducible Docker builds
vendor:
  cargo vendor --manifest-path build/risczero/kona/Cargo.toml --sync build/risczero/hokulea/Cargo.toml --sync build/risczero/hana/Cargo.toml build/risczero/vendor
  # kona-hardforks' build.rs reads op-core NUT bundles that cargo vendor omits; stage them.
  ./scripts/stage-nut-bundles.sh

# Build the release CLI with all DA providers (Ethereum blobs, EigenDA, Celestia)
build +ARGS="--bin kailua-cli --release -F prove -F disable-dev-mode -F eigen -F celestia --locked":
  cargo build {{ARGS}}

# Build the release CLI with all DA providers and experimental features
build-experimental +ARGS="--bin kailua-cli --release -F prove -F disable-dev-mode -F eigen -F celestia -F experimental --locked":
  cargo build {{ARGS}}

# Build the release CLI with Ethereum-blob DA only
build-kona +ARGS="--bin kailua-cli --release -F prove -F disable-dev-mode --locked":
  cargo build {{ARGS}}

# Build the Ethereum-blob-only release CLI with experimental features
build-kona-experimental +ARGS="--bin kailua-cli --release -F prove -F disable-dev-mode -F experimental --locked":
  cargo build {{ARGS}}

# Reproducibly rebuild all FPVM guests in Docker, then build the release CLI
build-fpvm +ARGS="--bin kailua-cli --release -F prove -F disable-dev-mode -F rebuild-fpvm -F eigen -F celestia --locked -vvv": vendor
  RISC0_USE_DOCKER=1 cargo build {{ARGS}}

# Reproducibly rebuild the experimental FPVM guests in Docker, then build the release CLI
build-fpvm-experimental +ARGS="--bin kailua-cli --release -F prove -F disable-dev-mode -F rebuild-fpvm -F eigen -F celestia -F experimental --locked -vvv": vendor
  RISC0_USE_DOCKER=1 cargo build {{ARGS}}

# Reproducibly rebuild only the kona FPVM guest in Docker, then build the release CLI
build-fpvm-kona +ARGS="--bin kailua-cli --release -F prove -F disable-dev-mode -F rebuild-fpvm --locked -vvv": vendor
  RISC0_USE_DOCKER=1 cargo build {{ARGS}}

# Reproducibly rebuild the experimental kona FPVM guest in Docker, then build the release CLI
build-fpvm-kona-experimental +ARGS="--bin kailua-cli --release -F prove -F disable-dev-mode -F rebuild-fpvm -F experimental --locked -vvv": vendor
  RISC0_USE_DOCKER=1 cargo build {{ARGS}}

# Build the kona guest workspace natively (no Docker, committed image IDs unchanged)
fpvm-kona:
  cargo build --manifest-path build/risczero/kona/Cargo.toml --locked --release -F disable-dev-mode

# Format the host workspace, the kona guest workspace, and the contracts
fmt-kona:
  cargo fmt --all
  cargo fmt --all --manifest-path build/risczero/kona/Cargo.toml
  forge fmt --root crates/contracts/foundry

# Format all Rust workspaces and the Solidity contracts
fmt:
  cargo fmt --all

  cargo fmt --all --manifest-path build/risczero/kona/Cargo.toml
  cargo fmt --all --manifest-path build/risczero/hokulea/Cargo.toml
  cargo fmt --all --manifest-path build/risczero/hana/Cargo.toml

  forge fmt --root crates/contracts/foundry

# Lint the host (default and full-featured) and all three guest workspaces, denying warnings
clippy:
  RISC0_SKIP_BUILD=true cargo clippy --bin kailua-cli --locked --all-targets -- -D warnings
  RISC0_SKIP_BUILD=true cargo clippy --bin kailua-cli --locked -F devnet -F eigen -F celestia -F experimental --all-targets -- -D warnings

  cargo clippy --manifest-path build/risczero/kona/Cargo.toml --locked --workspace --all --all-targets -- -D warnings
  cargo clippy --manifest-path build/risczero/hokulea/Cargo.toml --locked --workspace --all --all-targets -- -D warnings
  cargo clippy --manifest-path build/risczero/hana/Cargo.toml --locked --workspace --all --all-targets -- -D warnings

# Lint the host and the kona guest workspace only, denying warnings
clippy-kona:
  RISC0_SKIP_BUILD=true cargo clippy --bin kailua-cli --locked -- -D warnings
  RISC0_SKIP_BUILD=true cargo clippy --bin kailua-cli --locked -F devnet -- -D warnings

  cargo clippy --manifest-path build/risczero/kona/Cargo.toml --locked --workspace --all --all-targets -- -D warnings

# Measure kailua-kona test coverage (deliberately pinned nightly; see ci.yml before changing)
coverage +ARGS="":
  cargo +nightly-2026-03-26 llvm-cov -p kailua-kona --fail-uncovered-functions 0 --fail-uncovered-lines 10 {{ARGS}}
#  cargo +nightly-2026-03-26 llvm-cov -p kailua-kona --branch --fail-uncovered-functions 0 --fail-uncovered-lines 10 {{ARGS}}

# Measure coverage and open the HTML report
coverage-open: (coverage "--open")

# Fetch and patch the pinned optimism monorepo and Kurtosis package
devnet-fetch:
  ./scripts/devnet-fetch.sh

# Build a devnet CLI with all DA providers
devnet-build +ARGS="--bin kailua-cli -F devnet -F prove -F eigen -F celestia": (build ARGS)

# Build a devnet CLI with all DA providers and experimental features
devnet-build-experimental +ARGS="--bin kailua-cli -F devnet -F prove -F eigen -F celestia -F experimental": (build ARGS)

# Build a devnet CLI with Ethereum-blob DA only
devnet-build-kona +ARGS="--bin kailua-cli -F devnet -F prove": (build ARGS)

# Build an Ethereum-blob-only devnet CLI with experimental features
devnet-build-kona-experimental +ARGS="--bin kailua-cli -F devnet -F prove -F experimental": (build ARGS)

# Rebuild the FPVM guests, then build a devnet CLI with all DA providers
devnet-build-fpvm +ARGS="--bin kailua-cli -F devnet -F prove -F rebuild-fpvm -F eigen -F celestia": vendor (build ARGS)

# Rebuild the experimental FPVM guests, then build a devnet CLI
devnet-build-fpvm-experimental +ARGS="--bin kailua-cli -F devnet -F prove -F rebuild-fpvm -F eigen -F celestia -F experimental": vendor (build ARGS)

# Rebuild the kona FPVM guest, then build an Ethereum-blob-only devnet CLI
devnet-build-fpvm-kona +ARGS="--bin kailua-cli -F devnet -F prove -F rebuild-fpvm": vendor (build ARGS)

# Rebuild the experimental kona FPVM guest, then build a devnet CLI
devnet-build-fpvm-kona-experimental +ARGS="--bin kailua-cli -F devnet -F prove -F rebuild-fpvm -F experimental": vendor (build ARGS)

# Launch the local Kurtosis devnet and render its descriptor
devnet-up:
  ./scripts/devnet-up.sh

# Remove the devnet Kurtosis enclave
devnet-down:
  ./scripts/devnet-down.sh

# Remove the enclave and delete all devnet artifacts
devnet-clean:
  ./scripts/devnet-clean.sh

# Inspect the rollup configuration of the devnet
devnet-config target="debug" verbosity="" l1_rpc="" l2_rpc="" rollup_node_rpc="":
  #!/usr/bin/env bash
  set -euo pipefail
  source ./scripts/devnet-env.sh
  L1_RPC="$(devnet_resolve "{{l1_rpc}}" DEVNET_L1_RPC)"
  L2_RPC="$(devnet_resolve "{{l2_rpc}}" DEVNET_L2_RPC)"
  ROLLUP_NODE_RPC="$(devnet_resolve "{{rollup_node_rpc}}" DEVNET_OP_NODE_RPC)"
  ./target/{{target}}/kailua-cli config \
      --eth-rpc-url "$L1_RPC" \
      --op-geth-url "$L2_RPC" \
      --op-node-url "$ROLLUP_NODE_RPC" \
      --otlp-collector \
      {{verbosity}}

# Deploy Kailua to the devnet via fast-track (dev-mode proofs)
devnet-upgrade timeout="3600" advantage="60" target="debug" verbosity="" l1_rpc="" l2_rpc="" rollup_node_rpc="" vanguard="" deployer="" owner="" guardian="":
  #!/usr/bin/env bash
  set -euo pipefail
  source ./scripts/devnet-env.sh
  L1_RPC="$(devnet_resolve "{{l1_rpc}}" DEVNET_L1_RPC)"
  L2_RPC="$(devnet_resolve "{{l2_rpc}}" DEVNET_L2_RPC)"
  ROLLUP_NODE_RPC="$(devnet_resolve "{{rollup_node_rpc}}" DEVNET_OP_NODE_RPC)"
  VANGUARD="$(devnet_resolve "{{vanguard}}" DEVNET_VANGUARD_ADDRESS)"
  DEPLOYER="$(devnet_resolve "{{deployer}}" DEVNET_DEPLOYER_KEY)"
  OWNER="$(devnet_resolve "{{owner}}" DEVNET_OWNER_KEY)"
  GUARDIAN="$(devnet_resolve "{{guardian}}" DEVNET_GUARDIAN_KEY)"
  RISC0_DEV_MODE=1 ./target/{{target}}/kailua-cli fast-track \
      --eth-rpc-url "$L1_RPC" \
      --op-geth-url "$L2_RPC" \
      --op-node-url "$ROLLUP_NODE_RPC" \
      --starting-block-number 0 \
      --proposal-output-count 20 \
      --output-block-span 3 \
      --challenge-timeout {{timeout}} \
      --collateral-amount 1 \
      --deployer-key "$DEPLOYER" \
      --owner-key "$OWNER" \
      --guardian-key "$GUARDIAN" \
      --vanguard-address "$VANGUARD" \
      --vanguard-advantage {{advantage}} \
      --respect-kailua-proposals \
      {{verbosity}}

# Clean and relaunch the devnet
devnet-reset: devnet-clean devnet-up

# Run a proposer against the devnet
devnet-propose target="debug" verbosity="" l1_rpc="" l1_beacon_rpc="" l2_rpc="" rollup_node_rpc="" data_dir="{{devnet_propose_dir}}" proposer="":
  #!/usr/bin/env bash
  set -euo pipefail
  source ./scripts/devnet-env.sh
  L1_RPC="$(devnet_resolve "{{l1_rpc}}" DEVNET_L1_RPC)"
  L1_BEACON_RPC="$(devnet_resolve "{{l1_beacon_rpc}}" DEVNET_L1_BEACON_RPC)"
  L2_RPC="$(devnet_resolve "{{l2_rpc}}" DEVNET_L2_RPC)"
  ROLLUP_NODE_RPC="$(devnet_resolve "{{rollup_node_rpc}}" DEVNET_OP_NODE_RPC)"
  PROPOSER="$(devnet_resolve "{{proposer}}" DEVNET_PROPOSER_KEY)"
  ./target/{{target}}/kailua-cli propose \
      --eth-rpc-url "$L1_RPC" \
      --beacon-rpc-url "$L1_BEACON_RPC" \
      --op-geth-url "$L2_RPC" \
      --op-node-url "$ROLLUP_NODE_RPC" \
      --data-dir {{data_dir}} \
      --proposer-key "$PROPOSER" \
      {{verbosity}}

# Publish a faulty proposal on the devnet to test fault proving
devnet-fault offset parent target="debug" proposer="" verbosity="" l1_rpc="" l1_beacon_rpc="" l2_rpc="" rollup_node_rpc="":
  #!/usr/bin/env bash
  set -euo pipefail
  source ./scripts/devnet-env.sh
  L1_RPC="$(devnet_resolve "{{l1_rpc}}" DEVNET_L1_RPC)"
  L1_BEACON_RPC="$(devnet_resolve "{{l1_beacon_rpc}}" DEVNET_L1_BEACON_RPC)"
  L2_RPC="$(devnet_resolve "{{l2_rpc}}" DEVNET_L2_RPC)"
  ROLLUP_NODE_RPC="$(devnet_resolve "{{rollup_node_rpc}}" DEVNET_OP_NODE_RPC)"
  PROPOSER="$(devnet_resolve "{{proposer}}" DEVNET_FAULT_PROPOSER_KEY)"
  ./target/{{target}}/kailua-cli test-fault \
      --eth-rpc-url "$L1_RPC" \
      --beacon-rpc-url "$L1_BEACON_RPC" \
      --op-geth-url "$L2_RPC" \
      --op-node-url "$ROLLUP_NODE_RPC" \
      --proposer-key "$PROPOSER" \
      --fault-offset {{offset}} \
      --fault-parent {{parent}} \
      {{verbosity}}

# Run a validator against the devnet
devnet-validate fastforward="0" target="debug" verbosity="" l1_rpc="" l1_beacon_rpc="" l2_rpc="" rollup_node_rpc="" data_dir="{{devnet_validate_dir}}" validator="":
  #!/usr/bin/env bash
  set -euo pipefail
  source ./scripts/devnet-env.sh
  L1_RPC="$(devnet_resolve "{{l1_rpc}}" DEVNET_L1_RPC)"
  L1_BEACON_RPC="$(devnet_resolve "{{l1_beacon_rpc}}" DEVNET_L1_BEACON_RPC)"
  L2_RPC="$(devnet_resolve "{{l2_rpc}}" DEVNET_L2_RPC)"
  ROLLUP_NODE_RPC="$(devnet_resolve "{{rollup_node_rpc}}" DEVNET_OP_NODE_RPC)"
  VALIDATOR="$(devnet_resolve "{{validator}}" DEVNET_VALIDATOR_KEY)"
  ./target/{{target}}/kailua-cli validate \
      --fast-forward-target {{fastforward}} \
      --eth-rpc-url "$L1_RPC" \
      --beacon-rpc-url "$L1_BEACON_RPC" \
      --op-geth-url "$L2_RPC" \
      --op-node-url "$ROLLUP_NODE_RPC" \
      --data-dir {{data_dir}} \
      --validator-key "$VALIDATOR" \
      {{verbosity}}

# Compute a proof for a devnet block range
devnet-prove block_number block_count="1" target="debug" seq_window="50" verbosity="" data="{{devnet_data_dir}}" l1_rpc="" l1_beacon_rpc="" l2_rpc="" rollup_node_rpc="":
  #!/usr/bin/env bash
  set -euo pipefail
  source ./scripts/devnet-env.sh
  L1_RPC="$(devnet_resolve "{{l1_rpc}}" DEVNET_L1_RPC)"
  L1_BEACON_RPC="$(devnet_resolve "{{l1_beacon_rpc}}" DEVNET_L1_BEACON_RPC)"
  L2_RPC="$(devnet_resolve "{{l2_rpc}}" DEVNET_L2_RPC)"
  ROLLUP_NODE_RPC="$(devnet_resolve "{{rollup_node_rpc}}" DEVNET_OP_NODE_RPC)"
  just --justfile justfile prove "{{block_number}}" "{{block_count}}" "$L1_RPC" "$L1_BEACON_RPC" "$L2_RPC" "$ROLLUP_NODE_RPC" "{{data}}" "{{target}}" "{{seq_window}}" "{{verbosity}}"

# Run the withdrawal-assisting RPC server against the devnet
devnet-rpc socket="127.0.0.1:1337" target="debug" verbosity="" l1_rpc="" l1_beacon_rpc="" l2_rpc="" rollup_node_rpc="" data="{{devnet_data_dir}}":
  #!/usr/bin/env bash
  set -euo pipefail
  source ./scripts/devnet-env.sh
  L1_RPC="$(devnet_resolve "{{l1_rpc}}" DEVNET_L1_RPC)"
  L1_BEACON_RPC="$(devnet_resolve "{{l1_beacon_rpc}}" DEVNET_L1_BEACON_RPC)"
  L2_RPC="$(devnet_resolve "{{l2_rpc}}" DEVNET_L2_RPC)"
  ROLLUP_NODE_RPC="$(devnet_resolve "{{rollup_node_rpc}}" DEVNET_OP_NODE_RPC)"
  just --justfile justfile rpc "$L1_RPC" "$L1_BEACON_RPC" "$L2_RPC" "$ROLLUP_NODE_RPC" "{{socket}}" "{{data}}" "{{target}}" "{{verbosity}}"

# Continuously validity-prove a live rollup without any Kailua deployment
demo size l1_rpc l1_beacon_rpc l2_rpc rollup_node_rpc data="{{devnet_data_dir}}" target="release" verbosity="":
    ./target/{{target}}/kailua-cli demo \
          --eth-rpc-url {{l1_rpc}} \
          --beacon-rpc-url {{l1_beacon_rpc}} \
          --op-geth-url {{l2_rpc}} \
          --op-node-url {{rollup_node_rpc}} \
          --data-dir {{data}} \
          --num-blocks-per-proof {{size}} \
          {{verbosity}}

# Run the withdrawal-assisting RPC server against a live rollup
rpc l1_rpc l1_beacon_rpc l2_rpc rollup_node_rpc socket="127.0.0.1:1337" data="{{devnet_data_dir}}" target="release" verbosity="":
    ./target/{{target}}/kailua-cli rpc \
          --eth-rpc-url {{l1_rpc}} \
          --beacon-rpc-url {{l1_beacon_rpc}} \
          --op-geth-url {{l2_rpc}} \
          --op-node-url {{rollup_node_rpc}} \
          --socket-addr {{socket}} \
          --data-dir {{data}} \
          {{verbosity}}


# Benchmark proving performance over the heaviest blocks in a range
bench start length range count l1_rpc l1_beacon_rpc l2_rpc rollup_node_rpc data target="release" seq_window="50" verbosity="":
    ./target/{{target}}/kailua-cli benchmark \
          --eth-rpc-url {{l1_rpc}} \
          --beacon-rpc-url {{l1_beacon_rpc}} \
          --op-geth-url {{l2_rpc}} \
          --op-node-url {{rollup_node_rpc}} \
          --data-dir {{data}} \
          --bench-start {{start}} \
          --bench-length {{length}} \
          --bench-range {{range}} \
          --bench-count {{count}} \
          --seq-window {{seq_window}} \
          {{verbosity}}

# Export the FPVM guest binaries and print their image IDs
export-fpvm target="release" data="./build/risczero/src/bin" verbosity="":
  ./target/{{target}}/kailua-cli export {{verbosity}} --data-dir {{data}}

# Run the client program natively with the host program attached.
prove block_number block_count l1_rpc l1_beacon_rpc l2_rpc rollup_node_rpc data target="release" seq_window="50" verbosity="" system="0":
  #!/usr/bin/env bash

  case "{{system}}" in
    1|yes|true|TRUE) KAILUA_CLI=(kailua-cli) ;;
    *) KAILUA_CLI=("./target/{{target}}/kailua-cli") ;;
  esac

  L1_NODE_ADDRESS="{{l1_rpc}}"
  L1_BEACON_ADDRESS="{{l1_beacon_rpc}}"
  L2_NODE_ADDRESS="{{l2_rpc}}"
  OP_NODE_ADDRESS="{{rollup_node_rpc}}"

  L2_BLOCK_NUMBER={{block_number}}
  CLAIMED_L2_BLOCK_NUMBER=$((L2_BLOCK_NUMBER + {{block_count}} - 1))

  # Query the chain id
  echo "Fetching chain id"
  L2_CHAIN_ID=$(cast chain-id --rpc-url $L2_NODE_ADDRESS)

  # Get output root for block
  echo "Fetching data for block #$CLAIMED_L2_BLOCK_NUMBER..."
  CLAIMED_L2_OUTPUT_ROOT=$(cast rpc --rpc-url $OP_NODE_ADDRESS "optimism_outputAtBlock" $(cast 2h $CLAIMED_L2_BLOCK_NUMBER) | jq -r .outputRoot)
  # Get the info for the origin l1 block
  L1_ORIGIN_NUM=$(cast rpc --rpc-url $OP_NODE_ADDRESS "optimism_outputAtBlock" $(cast 2h $CLAIMED_L2_BLOCK_NUMBER) | jq -r .blockRef.l1origin.number)
  L1_HEAD=$(cast block --rpc-url $L1_NODE_ADDRESS $((L1_ORIGIN_NUM + {{seq_window}})) --json | jq -r .hash)

  # Get the info for the parent l2 block
  echo "Fetching data for parent of block #$L2_BLOCK_NUMBER..."
  AGREED_L2_OUTPUT_ROOT=$(cast rpc --rpc-url $OP_NODE_ADDRESS "optimism_outputAtBlock" $(cast 2h $((L2_BLOCK_NUMBER - 1))) | jq -r .outputRoot)
  AGREED_L2_HEAD=$(cast block --rpc-url $L2_NODE_ADDRESS $((L2_BLOCK_NUMBER - 1)) --json | jq -r .hash)

  echo "Running host program with zk client program..."
  "${KAILUA_CLI[@]}" prove {{verbosity}} \
    --op-node-address $OP_NODE_ADDRESS \
    --l1-head $L1_HEAD \
    --agreed-l2-head-hash $AGREED_L2_HEAD \
    --agreed-l2-output-root $AGREED_L2_OUTPUT_ROOT \
    --claimed-l2-output-root $CLAIMED_L2_OUTPUT_ROOT \
    --claimed-l2-block-number $CLAIMED_L2_BLOCK_NUMBER \
    --l2-chain-id $L2_CHAIN_ID \
    --l1-node-address $L1_NODE_ADDRESS \
    --l1-beacon-address $L1_BEACON_ADDRESS \
    --l2-node-address $L2_NODE_ADDRESS \
    --data-dir {{data}} \
    --native

# Show the input args for proving
query block_number l1_rpc l1_beacon_rpc l2_rpc rollup_node_rpc seq_window="50":
  #!/usr/bin/env bash

  L1_NODE_ADDRESS="{{l1_rpc}}"
  L1_BEACON_ADDRESS="{{l1_beacon_rpc}}"
  L2_NODE_ADDRESS="{{l2_rpc}}"
  OP_NODE_ADDRESS="{{rollup_node_rpc}}"

  L2_BLOCK_NUMBER={{block_number}}

  echo "Fetching data for block #$L2_BLOCK_NUMBER..."
  L1_ORIGIN_NUM=$(cast rpc --rpc-url $OP_NODE_ADDRESS "optimism_outputAtBlock" $(cast 2h $((L2_BLOCK_NUMBER - 1))) | jq -r .blockRef.l1origin.number)

  echo $L1_ORIGIN_NUM
  # L1 head
  cast block --rpc-url $L1_NODE_ADDRESS $((L1_ORIGIN_NUM + {{seq_window}})) --json | jq -r .hash
  # L2 hash
  cast block --rpc-url $L2_NODE_ADDRESS $((L2_BLOCK_NUMBER - 1)) --json | jq -r .hash
  # L2 Claim
  cast rpc --rpc-url $OP_NODE_ADDRESS "optimism_outputAtBlock" $(cast 2h $L2_BLOCK_NUMBER) | jq -r .outputRoot
  # L2 agreed output root
  cast rpc --rpc-url $OP_NODE_ADDRESS "optimism_outputAtBlock" $(cast 2h $((L2_BLOCK_NUMBER - 1))) | jq -r .outputRoot
  # L2 chain id
  cast chain-id --rpc-url $L2_NODE_ADDRESS

# Re-run a proof from cached data only, without any RPC endpoints
prove-offline block_number l1_head l2_hash l2_claim l2_output_root l2_chain_id data target="release" verbosity="":
  echo "Running host program with zk client program..."
  NUM_CONCURRENT_PREFLIGHTS=0 ./target/{{target}}/kailua-cli prove {{verbosity}} \
    --l1-head {{l1_head}} \
    --agreed-l2-head-hash {{l2_hash}} \
    --claimed-l2-output-root {{l2_claim}} \
    --agreed-l2-output-root {{l2_output_root}} \
    --claimed-l2-block-number {{block_number}} \
    --l2-chain-id {{l2_chain_id}} \
    --data-dir {{data}} \
    --native

# Run the Rust test suites with dev-mode proofs (RISC0_DEV_MODE=1)
test verbosity="":
    echo "Running cargo tests"
    RISC0_DEV_MODE=1 cargo test -F devnet

# Replay a recorded OP Sepolia proof from a local ./testdata cache
test-offline target="release" verbosity="": (prove-offline "16491249" "0x33a3e5721faa4dc6f25e75000d9810fd6c41320868f3befcc0c261a71da398e1" "0x09b298a83baf4c2e3c6a2e355bb09e27e3fdca435080e8754f8749233d7333b2" "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75" "0xa548f22e1aa590de7ed271e3eab5b66c6c3db9b8cb0e3f91618516ea9ececde4" "11155420" "./testdata/16491249" target verbosity)

# Delete proof and request artifacts from the working directory
cleanup:
    rm ./*.driver || true
    rm ./*.req || true
    rm ./*.fake || true


# Filter noisy tracing targets out of a proving log
grep-proving-log log:
    grep -v -e host_backend -e batch_queue -e kona_protocol -e R0VM -e block_builder -e batch_validator -e attributes_queue -e client_derivation_driver -e single_hint_handler -e kailua_common -e complete, -e client_blob_oracle -e agent -e channel_assembler -e kailua_sync -e "OUTPUT: " -e "CACHE "  {{log}}

# Tail a proving log with noise filtered out
follow-proving-log log:
    tail -f -n +0 {{log}} | just grep-proving-log --line-buffered

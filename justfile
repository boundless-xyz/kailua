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

vendor:
  cargo vendor --manifest-path build/risczero/kona/Cargo.toml --sync build/risczero/hokulea/Cargo.toml --sync build/risczero/hana/Cargo.toml build/risczero/vendor

build +ARGS="--bin kailua-cli --release -F prove -F disable-dev-mode -F eigen -F celestia --locked":
  cargo build {{ARGS}}

build-kona +ARGS="--bin kailua-cli --release -F prove -F disable-dev-mode --locked":
  cargo build {{ARGS}}

build-fpvm +ARGS="--bin kailua-cli --release -F prove -F disable-dev-mode -F rebuild-fpvm -F eigen -F celestia --locked -vvv": vendor
  RISC0_USE_DOCKER=1 cargo build {{ARGS}}

build-fpvm-kona +ARGS="--bin kailua-cli --release -F prove -F disable-dev-mode -F rebuild-fpvm --locked -vvv": vendor
  RISC0_USE_DOCKER=1 cargo build {{ARGS}}

fpvm-kona:
  cargo build --manifest-path build/risczero/kona/Cargo.toml --locked --release -F disable-dev-mode

fmt-kona:
  cargo fmt --all
  cargo fmt --all --manifest-path build/risczero/kona/Cargo.toml
  forge fmt --root crates/contracts/foundry

fmt:
  cargo fmt --all

  cargo fmt --all --manifest-path build/risczero/kona/Cargo.toml
  cargo fmt --all --manifest-path build/risczero/hokulea/Cargo.toml
  cargo fmt --all --manifest-path build/risczero/hana/Cargo.toml

  forge fmt --root crates/contracts/foundry

clippy:
  RISC0_SKIP_BUILD=true cargo clippy --bin kailua-cli --locked --all-targets -- -D warnings
  RISC0_SKIP_BUILD=true cargo clippy --bin kailua-cli --locked -F devnet -F eigen -F celestia --all-targets -- -D warnings

  cargo clippy --manifest-path build/risczero/kona/Cargo.toml --locked --workspace --all --all-targets -- -D warnings
  cargo clippy --manifest-path build/risczero/hokulea/Cargo.toml --locked --workspace --all --all-targets -- -D warnings
  cargo clippy --manifest-path build/risczero/hana/Cargo.toml --locked --workspace --all --all-targets -- -D warnings

clippy-kona:
  RISC0_SKIP_BUILD=true cargo clippy --bin kailua-cli --locked -- -D warnings
  RISC0_SKIP_BUILD=true cargo clippy --bin kailua-cli --locked -F devnet -- -D warnings

  cargo clippy --manifest-path build/risczero/kona/Cargo.toml --locked --workspace --all --all-targets -- -D warnings

coverage +ARGS="":
  cargo llvm-cov -p kailua-kona --fail-uncovered-functions 0 --fail-uncovered-lines 10 {{ARGS}}
#  cargo +nightly-2026-03-26 llvm-cov -p kailua-kona --branch --fail-uncovered-functions 0 --fail-uncovered-lines 10 {{ARGS}}

coverage-open: (coverage "--open")

devnet-fetch:
  ./scripts/devnet-fetch.sh

devnet-build +ARGS="--bin kailua-cli -F devnet -F prove -F eigen -F celestia": (build ARGS)

devnet-build-kona +ARGS="--bin kailua-cli -F devnet -F prove": (build ARGS)

devnet-build-fpvm +ARGS="--bin kailua-cli -F devnet -F prove -F rebuild-fpvm -F eigen -F celestia": vendor (build ARGS)

devnet-build-fpvm-kona +ARGS="--bin kailua-cli -F devnet -F prove -F rebuild-fpvm": vendor (build ARGS)

devnet-up:
  ./scripts/devnet-up.sh

devnet-down:
  ./scripts/devnet-down.sh

devnet-clean:
  ./scripts/devnet-clean.sh

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

devnet-reset: devnet-clean devnet-up

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

devnet-prove block_number block_count="1" target="debug" seq_window="50" verbosity="" data="{{devnet_data_dir}}" l1_rpc="" l1_beacon_rpc="" l2_rpc="" rollup_node_rpc="":
  #!/usr/bin/env bash
  set -euo pipefail
  source ./scripts/devnet-env.sh
  L1_RPC="$(devnet_resolve "{{l1_rpc}}" DEVNET_L1_RPC)"
  L1_BEACON_RPC="$(devnet_resolve "{{l1_beacon_rpc}}" DEVNET_L1_BEACON_RPC)"
  L2_RPC="$(devnet_resolve "{{l2_rpc}}" DEVNET_L2_RPC)"
  ROLLUP_NODE_RPC="$(devnet_resolve "{{rollup_node_rpc}}" DEVNET_OP_NODE_RPC)"
  just --justfile justfile prove "{{block_number}}" "{{block_count}}" "$L1_RPC" "$L1_BEACON_RPC" "$L2_RPC" "$ROLLUP_NODE_RPC" "{{data}}" "{{target}}" "{{seq_window}}" "{{verbosity}}"

devnet-rpc socket="127.0.0.1:1337" target="debug" verbosity="" l1_rpc="" l1_beacon_rpc="" l2_rpc="" rollup_node_rpc="" data="{{devnet_data_dir}}":
  #!/usr/bin/env bash
  set -euo pipefail
  source ./scripts/devnet-env.sh
  L1_RPC="$(devnet_resolve "{{l1_rpc}}" DEVNET_L1_RPC)"
  L1_BEACON_RPC="$(devnet_resolve "{{l1_beacon_rpc}}" DEVNET_L1_BEACON_RPC)"
  L2_RPC="$(devnet_resolve "{{l2_rpc}}" DEVNET_L2_RPC)"
  ROLLUP_NODE_RPC="$(devnet_resolve "{{rollup_node_rpc}}" DEVNET_OP_NODE_RPC)"
  just --justfile justfile rpc "$L1_RPC" "$L1_BEACON_RPC" "$L2_RPC" "$ROLLUP_NODE_RPC" "{{socket}}" "{{data}}" "{{target}}" "{{verbosity}}"

demo size l1_rpc l1_beacon_rpc l2_rpc rollup_node_rpc data="{{devnet_data_dir}}" target="release" verbosity="":
    ./target/{{target}}/kailua-cli demo \
          --eth-rpc-url {{l1_rpc}} \
          --beacon-rpc-url {{l1_beacon_rpc}} \
          --op-geth-url {{l2_rpc}} \
          --op-node-url {{rollup_node_rpc}} \
          --data-dir {{data}} \
          --num-blocks-per-proof {{size}} \
          {{verbosity}}

rpc l1_rpc l1_beacon_rpc l2_rpc rollup_node_rpc socket="127.0.0.1:1337" data="{{devnet_data_dir}}" target="release" verbosity="":
    ./target/{{target}}/kailua-cli rpc \
          --eth-rpc-url {{l1_rpc}} \
          --beacon-rpc-url {{l1_beacon_rpc}} \
          --op-geth-url {{l2_rpc}} \
          --op-node-url {{rollup_node_rpc}} \
          --socket-addr {{socket}} \
          --data-dir {{data}} \
          {{verbosity}}


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

export-fpvm target="release" data="./build/risczero/src/bin" verbosity="":
  ./target/{{target}}/kailua-cli export {{verbosity}} --data-dir {{data}}

# Run the client program natively with the host program attached.
prove block_number block_count l1_rpc l1_beacon_rpc l2_rpc rollup_node_rpc data target="release" seq_window="50" verbosity="":
  #!/usr/bin/env bash

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
  ./target/{{target}}/kailua-cli prove {{verbosity}} \
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

test verbosity="":
    echo "Running cargo tests"
    RISC0_DEV_MODE=1 cargo test -F devnet

test-offline target="release" verbosity="": (prove-offline "16491249" "0x33a3e5721faa4dc6f25e75000d9810fd6c41320868f3befcc0c261a71da398e1" "0x09b298a83baf4c2e3c6a2e355bb09e27e3fdca435080e8754f8749233d7333b2" "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75" "0xa548f22e1aa590de7ed271e3eab5b66c6c3db9b8cb0e3f91618516ea9ececde4" "11155420" "./testdata/16491249" target verbosity)

cleanup:
    rm ./*.driver || true
    rm ./*.req || true
    rm ./*.fake || true


grep-proving-log log:
    grep -v -e host_backend -e batch_queue -e kona_protocol -e R0VM -e block_builder -e batch_validator -e attributes_queue -e client_derivation_driver -e single_hint_handler -e kailua_common -e complete, -e client_blob_oracle -e agent -e channel_assembler -e kailua_sync -e "OUTPUT: " -e "CACHE "  {{log}}

follow-proving-log log:
    tail -f -n +0 {{log}} | just grep-proving-log --line-buffered

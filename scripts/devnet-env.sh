#!/usr/bin/env bash

devnet_load_env() {
  local descriptor="${1:-${KAILUA_DEVNET_DESCRIPTOR:-devnet/kurtosis-devnet.json}}"

  if [[ ! -f "$descriptor" ]]; then
    echo "Missing devnet descriptor at $descriptor. Run 'just devnet-up' first or pass explicit RPC/key args." >&2
    return 1
  fi

  local endpoint_filter='"\(.scheme // "http")://\(.host):\(.port)"'

  devnet_endpoint() {
    local jq_path="$1"
    jq -er "$jq_path | ${endpoint_filter}" "$descriptor"
  }

  devnet_wallet_key() {
    local alias="$1"
    jq -er --arg alias "$alias" '.l1.wallets[$alias].private_key' "$descriptor"
  }

  devnet_wallet_address() {
    local alias="$1"
    jq -er --arg alias "$alias" '.l1.wallets[$alias].address' "$descriptor"
  }

  export DEVNET_DESCRIPTOR="$descriptor"
  export DEVNET_L1_RPC
  DEVNET_L1_RPC="$(devnet_endpoint '.l1.nodes[0].services.el.endpoints.rpc')"
  export DEVNET_L1_BEACON_RPC
  DEVNET_L1_BEACON_RPC="$(devnet_endpoint '.l1.nodes[0].services.cl.endpoints.http')"
  export DEVNET_L2_RPC
  DEVNET_L2_RPC="$(devnet_endpoint '.l2[0].nodes[0].services.el.endpoints.rpc')"
  export DEVNET_OP_NODE_RPC
  DEVNET_OP_NODE_RPC="$(devnet_endpoint '.l2[0].nodes[0].services.cl.endpoints.http')"
  export DEVNET_DEPLOYER_KEY
  DEVNET_DEPLOYER_KEY="$(devnet_wallet_key 'deployer')"
  export DEVNET_OWNER_KEY
  DEVNET_OWNER_KEY="$(devnet_wallet_key 'owner')"
  export DEVNET_GUARDIAN_KEY
  DEVNET_GUARDIAN_KEY="$(devnet_wallet_key 'guardian')"
  export DEVNET_PROPOSER_KEY
  DEVNET_PROPOSER_KEY="$(devnet_wallet_key 'proposer')"
  export DEVNET_VALIDATOR_KEY
  DEVNET_VALIDATOR_KEY="$(devnet_wallet_key 'validator')"
  export DEVNET_FAULT_PROPOSER_KEY
  DEVNET_FAULT_PROPOSER_KEY="$(devnet_wallet_key 'fault-proposer')"
  export DEVNET_TRAIL_FAULT_PROPOSER_KEY
  DEVNET_TRAIL_FAULT_PROPOSER_KEY="$(devnet_wallet_key 'trail-fault-proposer')"
  export DEVNET_VANGUARD_ADDRESS
  DEVNET_VANGUARD_ADDRESS="$(devnet_wallet_address 'vanguard')"
  export DEVNET_ENV_LOADED=1
}

devnet_resolve() {
  local provided="$1"
  local env_name="$2"
  local descriptor="${3:-${KAILUA_DEVNET_DESCRIPTOR:-devnet/kurtosis-devnet.json}}"

  if [[ -n "$provided" ]]; then
    printf '%s\n' "$provided"
    return 0
  fi

  if [[ -z "${DEVNET_ENV_LOADED:-}" ]]; then
    devnet_load_env "$descriptor"
  fi

  printf '%s\n' "${!env_name}"
}

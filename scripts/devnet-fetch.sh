#!/usr/bin/env bash

set -euo pipefail

devnet_runtime_dir="devnet"
devnet_package_dir="devnet/optimism-package"
devnet_optimism_commit="3019251e80aa248e91743addd3e833190acb26f1"
devnet_package_commit="89e0b8cacab9f7e9f74d53b72d4870092825d577"

is_detached_at() {
  local dir="$1"
  local expected="$2"
  [[ -d "$dir/.git" ]] || return 1
  [[ "$(git -C "$dir" rev-parse HEAD 2>/dev/null)" == "$expected" ]] || return 1
  ! git -C "$dir" symbolic-ref -q HEAD >/dev/null 2>&1
}

mkdir -p "$devnet_runtime_dir"

if is_detached_at optimism "$devnet_optimism_commit"; then
  git -C optimism submodule update --init --recursive
elif [[ -d optimism/.git ]]; then
  git -C optimism fetch --depth 1 origin "$devnet_optimism_commit"
  git -C optimism checkout --detach "$devnet_optimism_commit"
  git -C optimism submodule update --init --recursive
else
  git clone --depth 1 --branch v1.16.7 --recursive https://github.com/ethereum-optimism/optimism.git
  git -C optimism checkout --detach "$devnet_optimism_commit"
  git -C optimism submodule update --init --recursive
fi

if is_detached_at "$devnet_package_dir" "$devnet_package_commit"; then
  :
elif [[ -d "$devnet_package_dir/.git" ]]; then
  git -C "$devnet_package_dir" fetch --depth 1 origin "$devnet_package_commit"
  git -C "$devnet_package_dir" checkout --detach "$devnet_package_commit"
else
  git clone --depth 1 https://github.com/ethpandaops/optimism-package.git "$devnet_package_dir"
  git -C "$devnet_package_dir" fetch --depth 1 origin "$devnet_package_commit"
  git -C "$devnet_package_dir" checkout --detach "$devnet_package_commit"
fi

./scripts/patch-optimism-package.sh "$devnet_package_dir"

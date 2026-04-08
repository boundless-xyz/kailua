#!/usr/bin/env bash

set -euo pipefail

devnet_enclave="kailua-eigenda-devnet"
devnet_runtime_dir="devnet"
devnet_descriptor="devnet/kurtosis-eigenda-devnet.json"
devnet_package_dir="devnet/optimism-package"
devnet_log="devnet/eigenda-devnet.log"
eigenda_local_image="kailua/eigenda-proxy:devnet"
eigenda_source_dir="devnet/eigenda-proxy-src"

ensure_local_eigenda_image() {
  if docker image inspect "$eigenda_local_image" >/dev/null 2>&1; then
    return
  fi

  if [[ ! -d "$eigenda_source_dir/.git" ]]; then
    echo "Cloning EigenDA proxy source into $eigenda_source_dir" >&2
    rm -rf "$eigenda_source_dir"
    git clone --depth 1 https://github.com/Layr-Labs/eigenda-proxy "$eigenda_source_dir"
  else
    echo "Updating EigenDA proxy source in $eigenda_source_dir" >&2
    git -C "$eigenda_source_dir" fetch --depth 1 origin main
    git -C "$eigenda_source_dir" reset --hard FETCH_HEAD
  fi

  echo "Building local EigenDA proxy image $eigenda_local_image" >&2
  docker build -t "$eigenda_local_image" "$eigenda_source_dir"
}

run_kurtosis() {
  python3 ./scripts/run-kurtosis-devnet.py \
    --package-dir "$devnet_package_dir" \
    --args-file "$PWD/kurtosis-eigenda.yaml" \
    --enclave "$devnet_enclave" \
    --log "$devnet_log" \
    --stall-timeout-secs 60
}

./scripts/devnet-fetch.sh
ensure_local_eigenda_image
kurtosis enclave rm -f "$devnet_enclave" >/dev/null 2>&1 || true
mkdir -p "$devnet_runtime_dir"
rm -f "$devnet_descriptor" "$devnet_log"

if run_kurtosis; then
  :
else
  status="$?"
  if [[ "$status" -eq 75 ]]; then
    echo "Kurtosis failed during package upload; restarting engine and retrying once." >&2
    kurtosis enclave rm -f "$devnet_enclave" >/dev/null 2>&1 || true
    kurtosis engine restart
    rm -f "$devnet_log"
    run_kurtosis
  else
    exit "$status"
  fi
fi

python3 ./scripts/render-devnet-descriptor.py --enclave "$devnet_enclave" --output "$devnet_descriptor"

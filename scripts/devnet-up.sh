#!/usr/bin/env bash

set -euo pipefail

devnet_enclave="kailua-devnet"
devnet_runtime_dir="devnet"
devnet_descriptor="devnet/kurtosis-devnet.json"
devnet_package_dir="devnet/optimism-package"
devnet_log="devnet/devnet.log"

run_kurtosis() {
  python3 ./scripts/run-kurtosis-devnet.py \
    --package-dir "$devnet_package_dir" \
    --args-file "$PWD/kurtosis.yaml" \
    --enclave "$devnet_enclave" \
    --log "$devnet_log" \
    --stall-timeout-secs 60
}

./scripts/devnet-fetch.sh
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

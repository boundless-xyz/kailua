#!/usr/bin/env bash

set -euo pipefail

kurtosis enclave rm -f kailua-eigenda-devnet >/dev/null 2>&1 || true
rm -rf devnet/kurtosis-eigenda-devnet.json devnet/eigenda-devnet.log devnet/data devnet/propose devnet/validate || true

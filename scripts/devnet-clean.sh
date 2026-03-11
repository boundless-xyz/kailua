#!/usr/bin/env bash

set -euo pipefail

kurtosis enclave rm -f kailua-devnet >/dev/null 2>&1 || true
rm -rf devnet/kurtosis-devnet.json devnet/devnet.log devnet/data devnet/propose devnet/validate || true

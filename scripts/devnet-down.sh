#!/usr/bin/env bash

set -euo pipefail

kurtosis enclave rm -f kailua-devnet >/dev/null 2>&1 || true

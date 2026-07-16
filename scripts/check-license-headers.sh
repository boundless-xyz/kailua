#!/usr/bin/env bash
# Verifies that every first-party Rust and Solidity source file starts with a
# "Copyright <years> Boundless Foundation, Inc." notice followed by the
# standard Apache-2.0 license header.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

# Generated files, rewritten by the FPVM image ID export flow.
EXEMPT=(
    "build/risczero/src/fpvm.rs"
    "build/risczero/src/fpvm-experimental.rs"
)

# Accepts a single year, a comma-separated list, or a range (e.g. "2024",
# "2024, 2025", or "2024 - 2026").
COPYRIGHT='^// Copyright 20[0-9]{2}((, 20[0-9]{2})*|( - 20[0-9]{2}))? Boundless Foundation, Inc\.$'
LICENSE_BODY='//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.'

failures=0
while IFS= read -r file; do
    for exempt in "${EXEMPT[@]}"; do
        [[ "$file" == "$exempt" ]] && continue 2
    done
    if ! head -n 1 "$file" | grep -Eq "$COPYRIGHT"; then
        echo "bad or missing copyright notice: $file"
        failures=$((failures + 1))
    elif [[ "$(sed -n '2,13p' "$file")" != "$LICENSE_BODY" ]]; then
        echo "bad or missing license header: $file"
        failures=$((failures + 1))
    fi
done < <(git ls-files -- '*.rs' '*.sol' ':!crates/contracts/foundry/lib/**')

if ((failures > 0)); then
    echo "license check failed for $failures file(s)"
    exit 1
fi
echo "license check passed"

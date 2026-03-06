#!/usr/bin/env bash

set -euo pipefail

package_dir="${1:?usage: patch-optimism-package.sh <package-dir>}"
expected_commit="89e0b8cacab9f7e9f74d53b72d4870092825d577"
patch_file="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/optimism-package-v1.16.7.patch"

if [[ ! -d "$package_dir/.git" ]]; then
  echo "Expected a git checkout at $package_dir" >&2
  exit 1
fi

head_commit="$(git -C "$package_dir" rev-parse HEAD)"
if [[ "$head_commit" != "$expected_commit" ]]; then
  echo "Refusing to patch $package_dir at $head_commit; expected $expected_commit" >&2
  exit 1
fi

if git -C "$package_dir" apply --check "$patch_file" >/dev/null 2>&1; then
  git -C "$package_dir" apply "$patch_file"
elif git -C "$package_dir" apply --reverse --check "$patch_file" >/dev/null 2>&1; then
  :
else
  echo "Failed to apply optimism-package compatibility patch at $package_dir" >&2
  exit 1
fi

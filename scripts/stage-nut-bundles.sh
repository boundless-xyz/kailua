#!/usr/bin/env bash
# Stage kona-hardforks' op-core NUT bundle JSON files into the vendored tree.
#
# kona-hardforks (kona-client/v1.5.2) ships a build.rs that reads
# `op-core/nuts/bundles/*_nut_bundle.json` from an ancestor of its crate directory
# to codegen `NutBundle` values. Those JSON files live at the optimism monorepo
# root (outside the crate), so `cargo vendor` — which only copies a crate's own
# files — omits them, and the (Docker) FPVM build panics with:
#   could not find op-core/nuts/bundles/karst_nut_bundle.json in any ancestor of ...
#
# Copy the bundles under the vendor root so the build.rs ancestor probe finds them
# (`build/risczero/vendor/op-core/...` is an ancestor of `.../vendor/kona-hardforks`
# and is included in the Docker bind-mount of the vendor directory).
set -euo pipefail

DEST="build/risczero/vendor/op-core/nuts/bundles"
CARGO_GIT="${CARGO_HOME:-$HOME/.cargo}/git/checkouts"

# Locate the kona-client/v1.5.2 optimism checkout (the only one carrying these
# bundles) by probing for the known file rather than guessing the rev hash.
src_file="$(find "$CARGO_GIT" -path '*/op-core/nuts/bundles/karst_nut_bundle.json' 2>/dev/null | head -1)"
if [ -z "$src_file" ]; then
  echo "stage-nut-bundles: could not find op-core/nuts/bundles in $CARGO_GIT;" >&2
  echo "                   ensure the optimism git source is checked out (run vendor first)." >&2
  exit 1
fi

mkdir -p "$DEST"
cp "$(dirname "$src_file")"/*_nut_bundle.json "$DEST/"
echo "stage-nut-bundles: staged $(ls "$DEST"/*_nut_bundle.json | wc -l | tr -d ' ') bundle(s) into $DEST"

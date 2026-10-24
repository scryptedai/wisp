#!/usr/bin/env bash
# Source from hooks / ci-check so every cargo invocation shares one target dir.
#   source "$(git rev-parse --show-toplevel)/contrib/build-env.sh"
set -euo pipefail

_SCRIPT="${BASH_SOURCE[0]:-$0}"
_BUILD_ENV_ROOT="$(cd "$(dirname "$_SCRIPT")/.." && pwd)"

export CARGO_TARGET_DIR="$(
  python3 "$_BUILD_ENV_ROOT/contrib/scripts/resolve-cargo-target-dir.py" "$_BUILD_ENV_ROOT"
)"
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-1}"
mkdir -p "$CARGO_TARGET_DIR"

# Pin fastembed model cache so repeated invariant runs do not re-download.
export FASTEMBED_CACHE_DIR="${FASTEMBED_CACHE_DIR:-${TMPDIR:-/tmp}/wisp-fastembed-cache}"

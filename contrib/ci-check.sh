#!/usr/bin/env bash
# Mirror .github/workflows/ci.yml (test job). Run from repo root before push.
# Includes ./contrib/coderabbit-check.sh (user-docs lint + rustdoc on touched pub items).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
# shellcheck source=build-env.sh
source "$ROOT/contrib/build-env.sh"

CLIPPY=(
  -D warnings
  -A clippy::type_complexity
  -A clippy::too_many_arguments
  -A clippy::manual_div_ceil
  -A clippy::collapsible_match
  -A clippy::redundant_closure
  -A clippy::unnecessary_get_then_check
  -A clippy::unnecessary_unwrap
)

if [[ "$(uname -s)" == Linux ]] && command -v mandoc >/dev/null; then
  mandoc -T lint -Wall contrib/man/wisp.1
elif [[ "$(uname -s)" == Linux ]]; then
  echo "skip: mandoc not installed (CI runs this on ubuntu-latest only)"
fi

cargo fmt --all -- --check
./contrib/coderabbit-check.sh
# Build debug (default features) once, then lint — clippy reuses the same dep artifacts.
cargo test --workspace --no-run
cargo clippy --all-targets -- "${CLIPPY[@]}"
cargo test --workspace
# Second feature graph (no semantic / onnx); cannot share with default features.
cargo test -p wisp-invariants --no-default-features
# Release profile is a separate cache lane from dev (expected full compile of release deps).
cargo build --release -p wisp-cli
cargo build --release -p wisp-cli --no-default-features

echo "ci-check: ok"

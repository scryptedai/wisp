#!/usr/bin/env bash
# Mirror .github/workflows/ci.yml (test job). Run from repo root before push.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# Always use the repo target dir so hooks do not fill ephemeral CI/sandbox caches.
export CARGO_TARGET_DIR="$ROOT/target"

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
cargo clippy --all-targets -- "${CLIPPY[@]}"
cargo test --workspace
FASTEMBED_CACHE_DIR="${FASTEMBED_CACHE_DIR:-${TMPDIR:-/tmp}/wisp-fastembed-cache}" \
  cargo test -p wisp-invariants
cargo test -p wisp-invariants --no-default-features
cargo build --release -p wisp-cli
cargo build --release -p wisp-cli --no-default-features

echo "ci-check: ok"

#!/usr/bin/env bash
# Build release wisp and install to ~/.cargo/bin (or CARGO_HOME/bin).
# Re-run after git pull to replace the global binary: same command, --force is implicit.
#
# Usage (from anywhere):
#   /path/to/wisp/contrib/scripts/install-local.sh
#   contrib/scripts/install-local.sh --verify    # fmt, clippy, invariants, then install
#   contrib/scripts/install-local.sh --no-semantic   # no default semantic / model download at runtime build

set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

export CARGO_TARGET_DIR="${WISP_CARGO_TARGET_DIR:-$root/target}"
mkdir -p "$CARGO_TARGET_DIR"

avail_kb="$(df -k "$CARGO_TARGET_DIR" | awk 'NR==2 {print $4}')"
if [[ "${avail_kb:-0}" -lt 8388608 ]]; then
  echo "Warning: less than ~8 GiB free on $(df -h "$CARGO_TARGET_DIR" | awk 'NR==2 {print $1}')." >&2
  echo "Release builds need headroom. Free disk space or set WISP_CARGO_TARGET_DIR to a volume with space." >&2
fi

verify=0
install_docs_prefix=""
features=(--features default)
while [[ $# -gt 0 ]]; do
  case "$1" in
    --verify)
      verify=1
      shift
      ;;
    --no-semantic)
      features=(--no-default-features)
      shift
      ;;
    --prefix)
      install_docs_prefix="$2"
      shift 2
      ;;
    -h | --help)
      sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "Unknown option: $1 (try --help)" >&2
      exit 1
      ;;
  esac
done

if [[ "$verify" -eq 1 ]]; then
  echo "==> cargo fmt --check"
  cargo fmt --all -- --check
  echo "==> cargo clippy (workspace)"
  cargo clippy --all-targets -- -D warnings \
    -A clippy::type_complexity \
    -A clippy::too_many_arguments \
    -A clippy::manual_div_ceil \
    -A clippy::collapsible_match \
    -A clippy::redundant_closure \
    -A clippy::unnecessary_get_then_check \
    -A clippy::unnecessary_unwrap
  echo "==> cargo test -p wisp-invariants"
  cargo test -p wisp-invariants "${features[@]}"
fi

echo "==> cargo build --release -p wisp-cli ${features[*]}"
cargo build --release -p wisp-cli "${features[@]}"

echo "==> cargo install --path crates/wisp-cli --force --locked ${features[*]}"
cargo install --path crates/wisp-cli --force --locked "${features[@]}"

bin="${CARGO_HOME:-$HOME/.cargo}/bin/wisp"
if [[ ! -x "$bin" ]]; then
  echo "Install finished but $bin is missing." >&2
  exit 1
fi

echo ""
echo "==> install topic documentation"
if [[ -n "$install_docs_prefix" ]]; then
  bash "$root/contrib/scripts/install-docs.sh" --prefix "$install_docs_prefix"
  bash "$root/contrib/scripts/install-man.sh" --prefix "$install_docs_prefix"
else
  bash "$root/contrib/scripts/install-docs.sh"
  bash "$root/contrib/scripts/install-man.sh"
fi

echo ""
echo "Installed: $bin"
"$bin" --version
"$bin" doc list 2>/dev/null | head -5 || true
case ":$PATH:" in
  *":${CARGO_HOME:-$HOME/.cargo}/bin:"*) ;;
  *)
    echo ""
    echo "Add Cargo's bin directory to PATH (once per shell profile):"
    echo '  export PATH="$HOME/.cargo/bin:$PATH"'
    ;;
esac

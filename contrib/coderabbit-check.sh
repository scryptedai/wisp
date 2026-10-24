#!/usr/bin/env bash
# Local checks aligned with CodeRabbit review practices (.coderabbit.yaml).
# Invoked from ci-check.sh and optionally pre-commit (--staged).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

STAGED=0
BASE="${CODERABBIT_BASE:-}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --staged)
      STAGED=1
      shift
      ;;
    --base)
      BASE="$2"
      shift 2
      ;;
    -h | --help)
      echo "Usage: contrib/coderabbit-check.sh [--staged] [--base REF]"
      echo "  --staged   diff against index (pre-commit)"
      echo "  --base REF merge base for branch diff (default: origin/main or main)"
      exit 0
      ;;
    *)
      echo "coderabbit-check: unknown option $1" >&2
      exit 1
      ;;
  esac
done

chmod +x "$ROOT/contrib/scripts/rust-doc-coverage.py" 2>/dev/null || true
chmod +x "$ROOT/contrib/scripts/user-docs-lint.sh" 2>/dev/null || true

if [[ "$STAGED" -eq 1 ]]; then
  "$ROOT/contrib/scripts/user-docs-lint.sh" --staged
  python3 "$ROOT/contrib/scripts/rust-doc-coverage.py" --staged
elif [[ -n "$BASE" ]]; then
  "$ROOT/contrib/scripts/user-docs-lint.sh" --base "$BASE"
  python3 "$ROOT/contrib/scripts/rust-doc-coverage.py" --base "$BASE"
else
  "$ROOT/contrib/scripts/user-docs-lint.sh"
  python3 "$ROOT/contrib/scripts/rust-doc-coverage.py"
fi

echo "coderabbit-check: ok"

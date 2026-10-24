#!/usr/bin/env bash
# User-facing docs: present-tense contract (see .cursor/rules/docs-cli-man.mdc).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

STAGED=0
BASE=""
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
      sed -n '2,5p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "user-docs-lint: unknown option $1" >&2
      exit 1
      ;;
  esac
done

if [[ -z "$BASE" ]]; then
  for candidate in origin/main main; do
    if git rev-parse --verify "$candidate" >/dev/null 2>&1; then
      BASE="$candidate"
      break
    fi
  done
fi

collect_files() {
  if [[ "$STAGED" -eq 1 ]]; then
    git diff --cached --name-only --diff-filter=ACMR
  elif [[ -n "$BASE" ]]; then
    git diff --name-only --diff-filter=ACMR "${BASE}...HEAD"
  fi
}

TARGETS=()
while IFS= read -r f; do
  [[ -z "$f" ]] && continue
  case "$f" in
    docs/references/*) continue ;;
    docs/DECISIONS.md) continue ;;
    docs/*.md) TARGETS+=("$f") ;;
    README.md | CONTRIBUTING.md | contrib/man/wisp.1 | contrib/README.md)
      TARGETS+=("$f")
      ;;
  esac
done < <(collect_files)

if [[ ${#TARGETS[@]} -eq 0 ]]; then
  echo "user-docs-lint: no changed user docs (skip)"
  exit 0
fi

PATTERNS=(
  'Previously '
  'previously '
  'instead of synchronously'
  'backward compatible'
  'migration from older'
  'we changed '
  'we now '
)

FAIL=0
for f in "${TARGETS[@]}"; do
  [[ -f "$f" ]] || continue
  for pat in "${PATTERNS[@]}"; do
    if grep -n -E "$pat" "$f" >/dev/null 2>&1; then
      echo "user-docs-lint: $f matches forbidden phrase: $pat" >&2
      grep -n -E "$pat" "$f" >&2 || true
      FAIL=1
    fi
  done
done

if [[ "$FAIL" -ne 0 ]]; then
  echo "user-docs-lint: FAIL — write what the product does now (see docs-cli-man rule)" >&2
  exit 1
fi

echo "user-docs-lint: ok (${#TARGETS[@]} file(s))"

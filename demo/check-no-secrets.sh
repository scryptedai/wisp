#!/usr/bin/env bash
# Fail if a Scrypted bearer token or demo .env is staged for commit.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

FAIL=0

if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  if git diff --cached --name-only | grep -qE '^demo/\.env'; then
    echo "error: demo/.env must not be committed" >&2
    FAIL=1
  fi
  # scrypted_ + substantial secret material (not the placeholder in .env.example)
  if git diff --cached -U0 | grep -E 'scrypted_[A-Za-z0-9_-]{24,}' | grep -v 'scrypted_your_token' | grep -v 'scrypted_…'; then
    echo "error: staged diff looks like a Scrypted bearer token" >&2
    FAIL=1
  fi
fi

# Working tree scan under demo/ (skip .env, example, pdf)
PATTERN='scrypted_[A-Za-z0-9_-]{24,}'
if command -v rg >/dev/null; then
  HITS=$(rg -n "$PATTERN" demo/ -g '!.env' -g '!.env.*' -g '!*.pdf' -g '!check-no-secrets.sh' 2>/dev/null || true)
else
  HITS=$(grep -RInE "$PATTERN" demo/ --exclude='.env' --exclude='*.pdf' 2>/dev/null || true)
fi
if [[ -n "$HITS" ]]; then
  echo "$HITS"
  echo "error: possible Scrypted token under demo/" >&2
  FAIL=1
fi

if [[ "$FAIL" -ne 0 ]]; then
  exit 1
fi
echo "demo/: no Scrypted bearer tokens detected"

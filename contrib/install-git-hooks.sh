#!/usr/bin/env bash
# Install contrib/git-hooks into .git/hooks (symlinks). Safe to re-run.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HOOK_SRC="$ROOT/contrib/git-hooks"
GIT_HOOKS="$ROOT/.git/hooks"

if [[ ! -d "$ROOT/.git" ]]; then
  echo "install-git-hooks: not a git repository: $ROOT" >&2
  exit 1
fi

mkdir -p "$GIT_HOOKS"
for name in pre-commit pre-push; do
  src="$HOOK_SRC/$name"
  dst="$GIT_HOOKS/$name"
  if [[ ! -f "$src" ]]; then
    echo "install-git-hooks: missing $src" >&2
    exit 1
  fi
  chmod +x "$src"
  ln -sf "$src" "$dst"
  echo "installed $dst -> $src"
done

echo "Hooks active. pre-commit: cargo fmt --check. pre-push: ./contrib/ci-check.sh"

#!/usr/bin/env bash
# Strip Cursor / agent attribution from every commit message in this clone.
# Requires: git-filter-repo (pip install git-filter-repo)
#
# Usage (from repo root, with a clean working tree):
#   contrib/scripts/strip-commit-attribution.sh
#   git remote add origin <url>   # filter-repo removes remotes
#   git push --force-with-lease origin --all
#   git push --force-with-lease origin --tags

set -euo pipefail

root="$(git rev-parse --show-toplevel)"
cd "$root"

if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "Working tree must be clean before rewriting history." >&2
  exit 1
fi

if ! command -v git-filter-repo >/dev/null 2>&1; then
  echo "Install git-filter-repo (e.g. pip install git-filter-repo)." >&2
  exit 1
fi

before="$(git rev-list --all | wc -l | tr -d ' ')"
echo "Commits before scrub: $before"

git filter-repo --force --message-callback '
import re

lines = message.split(b"\n")
kept = []
for line in lines:
    if re.match(rb"(?i)^co-authored-by:\s*cursor\b", line):
        continue
    if re.match(rb"(?i)^co-authored-by:.*cursoragent@", line):
        continue
    if re.match(rb"(?i)^generated-by:\s*cursor\b", line):
        continue
    if re.search(rb"(?i)made with.*cursor", line):
        continue
    kept.append(line)
out = b"\n".join(kept).rstrip()
message = (out + b"\n") if out else message
'

after="$(git rev-list --all | wc -l | tr -d ' ')"
echo "Commits after scrub: $after"
echo "Done. Re-add origin and force-push all refs if hashes changed."

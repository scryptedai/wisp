#!/usr/bin/env bash
# Copy wisp topic documentation to a share directory (FHS or XDG).
#
# Usage:
#   contrib/scripts/install-docs.sh
#   contrib/scripts/install-docs.sh --prefix /usr/local   # → /usr/local/share/doc/wisp
#   WISP_DOC_ROOT=~/my/docs contrib/scripts/install-docs.sh

set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
prefix=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix)
      prefix="$2"
      shift 2
      ;;
    -h | --help)
      sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
done

if [[ -n "$prefix" ]]; then
  dest="$prefix/share/doc/wisp"
else
  dest="${WISP_DOC_ROOT:-${XDG_DATA_HOME:-$HOME/.local/share}/wisp/doc}"
fi

mkdir -p "$dest"
rsync -a --delete "$root/docs/" "$dest/"
for f in DECISIONS.md AGENTS.md; do
  cp "$root/$f" "$dest/"
done

echo "Installed documentation: $dest"
echo "Topics: wisp doc list"
if [[ -z "${WISP_DOC_ROOT:-}" ]]; then
  echo "Optional: export WISP_DOC_ROOT=\"$dest\""
fi

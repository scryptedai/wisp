#!/usr/bin/env bash
# Install wisp.1 for man(1).
#
# Usage:
#   contrib/scripts/install-man.sh
#   contrib/scripts/install-man.sh --prefix /usr/local

set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
prefix=""
src="$root/contrib/man/wisp.1"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix)
      prefix="$2"
      shift 2
      ;;
    -h | --help)
      sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
done

if [[ ! -f "$src" ]]; then
  echo "Missing $src" >&2
  exit 1
fi

if [[ -n "$prefix" ]]; then
  dest="$prefix/share/man/man1"
else
  dest="${WISP_MAN_ROOT:-${XDG_DATA_HOME:-$HOME/.local/share}/man}/man1"
fi

mkdir -p "$dest"
cp "$src" "$dest/wisp.1"
chmod 644 "$dest/wisp.1"

echo "Installed man page: $dest/wisp.1"
if [[ -z "$prefix" ]]; then
  man_base="$(dirname "$(dirname "$dest")")"
  echo "Ensure man can find it, e.g. in ~/.zshrc:"
  echo "  export MANPATH=\"$man_base:\${MANPATH:-\$(manpath 2>/dev/null || true)}\""
  echo "Or: man -M $man_base wisp"
fi

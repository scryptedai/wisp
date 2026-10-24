#!/usr/bin/env bash
# Run wisp from this repository without installing (debug build, slower startup).
#
# Usage:
#   contrib/scripts/wisp-dev.sh status
#   contrib/scripts/wisp-dev.sh --json cat notes/foo.md

set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
exec cargo run -q -p wisp-cli -- "$@"

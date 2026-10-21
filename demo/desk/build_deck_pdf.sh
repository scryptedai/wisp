#!/usr/bin/env bash
# Build pitch PDF from deck.marp.md + deck-theme.css (requires Node/npx).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
OUT="${ROOT}/wisp-desk-demo-pitch.pdf"
cd "$ROOT"
npx --yes @marp-team/marp-cli@4.5.1 \
  deck.marp.md \
  --theme-set ./deck-theme.css \
  --pdf \
  --allow-local-files \
  -o "$OUT"
echo "Wrote $OUT"
ls -lh "$OUT"

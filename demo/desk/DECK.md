# wisp desk demo — explainer deck

**PDF pitch deck:** run `./demo/desk/build_deck_pdf.sh` → [`wisp-desk-demo-pitch.pdf`](wisp-desk-demo-pitch.pdf) (sources: [`deck.marp.md`](deck.marp.md), [`deck-theme.css`](deck-theme.css)).

Speaker notes inline as blockquotes. Slide copy matches [`deck-slides.json`](deck-slides.json) and the interactive canvas in Cursor (`desk-demo-explainer.canvas.tsx`).

---

## Title — Two agents, one book

- Local-first OKF bundle: section changesets, encrypted cold store, hot SQLite
- Run: `python3 demo/desk/desk_demo.py all --fresh`
- Peers: `demo/workspaces/primary` + `demo/workspaces/replica` (gitignored)

> Accountable autonomy: two agents share one book, not two RAG silos.

---

## Problem

| Chat + vectors | wisp |
|----------------|------|
| Forked summaries | Same paths, same sections |
| No merge | Scope conflicts + `--resolutions` |
| Weak erasure story | `erase` + sync mirror |

---

## Step 0 — SEED

1. Requires `docs/test` with SPY week (your learning ingest).
2. `--fresh` → `demo/workspaces/primary`, clone `demo/workspaces/replica`.
3. Build CLI: `cargo build -p wisp-cli --release`.

```sh
python3 demo/desk/desk_demo.py seed --fresh
```

---

## Act 1 — Live stub + provenance

| Step | CLI / behavior |
|------|----------------|
| Feed | `demo/desk/fixtures/oct14_feed.json` |
| Write | `add-section` on `observations/2026-10-14-spy-options-session` |
| Link | `--input observations/2026-10-13-spy-options-session` |
| Prove | `log -n 1`, `why <path> --depth 1` |

Optional LLM draft (`--llm`):

```sh
# Copy demo/.env.example → demo/.env and set SCRYPTED_BEARER_TOKEN (never commit)
python3 demo/desk/desk_demo.py act1 --llm
```

Uses Scrypted AI `https://api.scrypted.ai/generations/text/nova-pro` (same pattern as delula `invokeTextGeneration`).

---

## Act 2 — Bear vs Hawk

1. Branch `task/bear-demo`; actor `process:bear/1.0` updates Wed **strategy** section (fade).
2. On `main`, `process:hawk/1.0` updates same section (break confirmed).
3. `merge task/bear-demo --into main` → **conflict** (scope_conflict).
4. Write `.wisp/demo-resolutions.json` with `take: "theirs"` → Bear text wins.
5. `search fade breakdown` — merged strategy ranks first.

---

## Act 3 — Erase + replica

1. `search max pain` — Fri **Options** section top hit.
2. `erase observations/2026-10-16-spy-options-session#<Options> --reason policy -y --discard`.
3. Managed `.md` no longer contains “max pain”; journal may still mention it.
4. `remote add` / `push` / replica `pull` — deletions mirror.

**Last verified run (2026-10-21):** `erased observation still in search: False`, `managed file contains 'max pain': False`, replica pull OK.

---

## Act 4 — Auditor

- `fsck` — integrity pass
- `log -n 5` — actors and ops in order
- Question → `search --mode auto` + `cat` — grounded snippets (demo uses Friday flush / journal)

---

## Close

- **8 min narration:** `python3 demo/desk/desk_demo.py narrate`
- **Regenerate slide JSON via LLM:** `python3 demo/desk/generate_deck.py` (needs `SCRYPTED_BEARER_TOKEN`)
- **Rotate tokens** if exposed; use env only.

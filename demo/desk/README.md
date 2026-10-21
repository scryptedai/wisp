# Desk demo — “Two agents, one book”

Live desk demo: **provenance**, **merge conflict**, **erase + replica sync**, **audit replay** on the SPY options learning bundle.

Parent index: [`../README.md`](../README.md) (secrets, workspaces).

## Prereqs

```sh
cargo build -p wisp-cli --release
# Seed content: docs/test with SPY week recorded (see docs/SCRATCH-STORE.md)
export WISP_ACTOR=human:tim
```

Optional LLM for Act 1: copy [`../.env.example`](../.env.example) → **`../.env`** and set `SCRYPTED_BEARER_TOKEN` (never commit `.env`).

## Run (full script, ~1 min)

```sh
python3 demo/desk/desk_demo.py all --fresh
python3 demo/desk/desk_demo.py narrate   # speaker notes
```

`docs/test` must already contain the recorded SPY week.

Workspaces (gitignored): **`demo/workspaces/primary`** and **`demo/workspaces/replica`**. Source bundle defaults to **`docs/test`**.

## Acts

| Act | What it shows |
|-----|----------------|
| **1** | Hawk agent `add-section` on Wed observation with `--input` prior day; `log` / `why` |
| **2** | Bear vs Hawk `update` same strategy section on two refs → `merge` conflict → `--resolutions` |
| **3** | `erase` Fri “Options” section; `search` before/after; `push`/`pull` to replica |
| **4** | `fsck`, `log`, grounded Q&A from `search` + `cat` |

## Options

```sh
WISP_BIN=/path/to/wisp python3 demo/desk/desk_demo.py all --fresh
python3 demo/desk/desk_demo.py act2 --demo demo/workspaces/primary
python3 demo/desk/desk_demo.py act1 --llm
```

## Explainer deck

- **PDF:** `cd demo/desk && ./build_deck_pdf.sh` → `wisp-desk-demo-pitch.pdf` (gitignored; rebuild locally)
- Markdown: [`DECK.md`](DECK.md) · JSON: [`deck-slides.json`](deck-slides.json)
- Regenerate slide copy: `python3 demo/desk/generate_deck.py` (uses `demo/.env`)

## Reset

```sh
rm -rf demo/workspaces
```

If you still have old paths: `rm -rf docs/demo docs/demo-replica`

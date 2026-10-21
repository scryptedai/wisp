---
marp: true
theme: wisp-pitch
size: 16:9
paginate: true
header: "wisp · desk demo"
footer: "Scrypted Community Release · GPLv2"
style: |
  section { justify-content: flex-start; }
---

<!-- _class: lead -->

<div class="kicker">Accountable autonomy</div>

# Two agents,<br/>one book

<p class="subtitle">Live desk demo on a SPY options OKF bundle — merge, erase, provenance, sync</p>

<div class="badge-row">
<span class="badge">Section ledger</span>
<span class="badge">Actor-attributed</span>
<span class="badge">Local-first</span>
</div>

> Agents commit to the same ledger — not parallel RAG silos.

---

<!-- _class: problem -->

## Why not two RAG silos?

| Chat + vectors | **wisp** |
| --- | --- |
| Forked summaries | Same paths, same sections |
| No merge semantics | Scope conflicts + `--resolutions` |
| Weak erasure | `erase` + sync mirror |
| Audit = chat export | `log`, `why`, `fsck` on the bundle |

---

<!-- _class: problem -->

## The gap

- Duplicate narratives with **no merge** or losing-side replay
- No per-section **provenance** (`--input`, `why`) tied to commits
- Erasure and compliance as afterthoughts

*One OKF bundle every agent reads and writes through `wisp`.*

---

<!-- _class: act -->

## Step 0 — SEED

- Source: recorded week in `docs/test` (gitignored scratch store)
- `--fresh` → `demo/workspaces/primary` + clone `demo/workspaces/replica`
- Prereq: `cargo build -p wisp-cli --release`

```sh
python3 demo/desk/desk_demo.py all --fresh
```

---

<!-- _class: act-alt compact -->

## Act 1 — Live stub + provenance

- Fixture: `fixtures/oct14_feed.json` (stub tick, not live data)
- `add-section` on Wed observation after **Cash session**
- `--input` links prior Tue observation · then `log` and `why`

```sh
wisp add-section observations/2026-10-14-spy-options-session \
  --input observations/2026-10-13-spy-options-session -m 'Hawk: intraday break stub @ 10:35'
wisp log -n 1 && wisp why observations/2026-10-14-spy-options-session --depth 1
```

---

<!-- _class: act compact -->

## Act 2 — Bear vs Hawk

**Target:** `strategy/2026-10-14-ex-post-optimal-play` — one section, two stories

- `process:bear/1.0` on `task/bear-demo` — fade breakdown
- `process:hawk/1.0` on `main` — break 772 confirmed
- Merge **conflict** → `demo-resolutions.json` with `take: theirs` (Bear wins)
- Ledger keeps **both** histories; merge node on `process:wisp-merge`

---

<!-- _class: act-alt -->

## Act 3 — Erase + replica sync

- `erase` Fri **Options** section (`--reason policy`, `--discard`)
- Search `max pain`: Fri file cleared; journal may still mention phrase
- `remote add` · `push` · replica `pull` — managed markdown mirrors

**Verified:** Fri obs absent from search · replica pull OK

---

<!-- _class: act -->

## Act 4 — Auditor replay

- `fsck` — integrity pass (8 changesets, no problems)
- `log -n 5` — erase, merge, hawk, bear, act1 with **actors**
- Grounded Q&A: `search --mode auto` + `cat` — no chat log required

*Compliance reads the ledger; investigators don't reconstruct from Slack.*

---

<!-- _class: close -->

## What to remember

- **Actors:** `human:*`, `process:bear/1.0`, `process:hawk/1.0`, `process:wisp-merge`
- Section merge + erase + `file://` sync in **~1 minute** live
- Run: `python3 demo/desk/desk_demo.py narrate`

<div class="badge-row">
<span class="badge">demo/desk</span>
<span class="badge">github.com/scryptedai/wisp</span>
</div>

# wisp 0.1.0 — Community Release

A local-first, differential knowledge store for [Open Knowledge Format](https://cloud.google.com/) (OKF) bundles.

This repository is **Scrypted’s GPLv2 Community Release** of wisp: source is here for use, study, modification, and redistribution under [LICENSE](LICENSE). Scrypted may offer other editions or licenses separately; **this tree is the community edition**.

Copyright © 2026 Scrypted. Author: Tim Cotten (tcotten@scrypted.ai).

wisp stores a bundle as an append-only log of **section-level** changes with encrypted, destroyable payloads, and materializes a conformant OKF directory on demand. It is built for fleets of agents: many writers commit without locks, every change carries provenance, and any section, concept, or group can be provably erased while the integrity chain still verifies.

Release **0.1.0** behavior is specified in **[docs/SEARCH.md](docs/SEARCH.md)** (retrieval), **[docs/EXTENSIONS.md](docs/EXTENSIONS.md)** (optional product hooks), **[DECISIONS.md](DECISIONS.md)** (OKF/store deltas), and **[docs/MERGE.md](docs/MERGE.md)**.

## Contributing

Fork, use **git worktrees** for parallel branches, and open PRs — see **[CONTRIBUTING.md](CONTRIBUTING.md)**. AI/review tools: **[AGENTS.md](AGENTS.md)**.

## Build

```sh
cargo build --release        # one static binary: target/release/wisp
cargo test                   # unit + engine invariant tests (§17)
cargo test -p wisp-invariants  # OKF corpus + search IR (semantic on by default; first run may fetch ~128MiB model)
```

Rust 1.88+. No services, no daemons: SQLite (bundled), XChaCha20-Poly1305, Ed25519, FTS5.

## Five-minute tour

```sh
cd my-bundle                          # a directory of OKF markdown
wisp init                             # .wisp/, repo scope, default policies, main
export WISP_ACTOR=human:tim
wisp record -m "Initial bundle"       # working-tree edits → one changeset
cat tables/orders.md                  # wisp_id and section anchors were written in
```

Agents write without touching files (§11.3):

```sh
# get a base version
wisp cat tables/orders.md --json
# commit against it; disjoint edits never conflict
wisp --actor sl-bot/0.3 update tables/orders.md#01JAD2V7… --base 01JAB7Q3M9… \
     --content "Rows are append-only." --input observations/2026-10-17.md -m "Sync"
# a real collision returns a JSON conflict object (exit 3) with base/theirs/yours
```

Context for a model (§14):

```sh
wisp search "door behavior"                 # auto: BM25 + fuzzy → RI → semantic when built in
wisp search "RAM depleted" --local-only   # same through RI; no neural model load
wisp pack tables/orders.md --budget 4000    # JSON, within a token budget, reports what it omitted
wisp why tables/orders.md#01JAD2V7… --depth 2   # actors, origins, the input graph
```

Erase, and prove it (§10):

```sh
wisp erase notes/standup.md#01JEF4X… --reason policy        # lists what it would destroy
wisp erase notes/standup.md#01JEF4X… --reason policy --yes  # key gone; hot tier purged; marker left
wisp fsck                                                   # digest chain unchanged, still passes
```

Every command takes `--json` and returns exactly one object: `{ok, command, data}` or `{ok:false, error:{code, message, details}}`. Exit codes are stable (§16): 3 conflict, 4 refused by policy, 5 validation, 7 retryable write timeout, 8 integrity, 9 key unavailable, 10 erase pending.

## Layout

```
crates/
  wisp-cli       argument parsing, output formatting, exit codes
  wisp-engine    record, checkout, optimistic commits, headless writes, erase, queries
  wisp-model     IDs, ops, changes, changesets, canonical CBOR digest, errors
  wisp-store     ledger.db (append-only, insert-only triggers), hot.db (FTS5), payload files
  wisp-crypto    scope keys, payload envelope, HMAC commitments, keystore, Ed25519
  wisp-parse     lossless frontmatter entries, sectioning, anchors, links, footnotes
  wisp-match     section identity rules (§7.5), concept identity (§7.4)
  wisp-profile   okf-0.2 and okf-0.1 profiles, renderers, migration plan
  wisp-feed, wisp-sub-tokens, wisp-sub-embed, wisp-merge, wisp-sync, wisp-git   (see Status)
```

On disk (§15): `.wisp/manifest.toml`, `.wisp/ledger.db`, `.wisp/payloads/ab/<id>`, `.wisp/keys/keystore.db`, `.wisp/cache/refs/<ref-id>/hot.db` (rebuildable, excluded from backups), `.wisp/checkout.idx`.

## Keys

The keystore unlocks from, in order: `WISP_MASTER_KEY` (64 hex chars; CI), `--passphrase` (Argon2id, m=64 MiB, t=3, p=1), or `.wisp/keys/master.key` (created at `init`, mode 0600). `wisp backup --with-keys <dir>` writes the keystore to a separate destination; keep its retention short (§10.6).

## Status

| Milestone | State |
|---|---|
| M1 Parse (frontmatter, sectioning, anchors, identity) | done |
| M2 Cold tier and crypto (ledger, payload envelope, keystore, staging, changeset signing) | done |
| M3 Hot tier, optimistic commits, record, checkout, views, okf-0.2 | done |
| M4 Lifecycle and erase (retract/restore, groups, erase, fsck, policies, revert, backup) | done |
| M5 Feed and search | feed, search (trigram/RI/semantic), pack, tokens (approx codec), external subscriber host done |
| M6 Profiles and migration | okf-0.1 profile, 0.1→0.2 migration plan, down-export (`export --profile 0.1`) done |
| M7 Concurrency and branches | headless writes, read assertions, branches, switch, worktrees, merge (fast-forward, disjoint replay, erase-wins, layered/nested replay, change-chain resolutions, `wisp mv` conflicts, `--resolutions`, `--abort`); see `docs/MERGE.md`, `merge_playbook` + `merge_rules` |
| M8 Sync, git interop, release | `file://` + HTTPS pack + SSH/rsync remotes, `export --archive` tar.zst, `import --git` (snapshot or `--history`), `export --git`, `serve --stdio` skeleton |

Invariants exercised in `cargo test`: I1, I2, I3 (parse layer and strip-anchors round-trip), I4, I5, I7, I12, I13, I16, I17, I19, I21, I22, I23, I24, I25 (staging flush on open); §11.5 merge (`merge_playbook`, `merge_rules`, `contrib/merge-rules/cases.toml`) and §11.6 sync roundtrip integration tests in `wisp-invariants`.

The **`wisp-corpus`** crate holds versioned OKF bundles (including a **SPY calls / news-signals** desk playbook under `bundles/spy-options/`), parse fixtures, and matcher scenarios (`catalog.toml`). **`wisp-invariants`** runs them on every CI build: record → validate → fsck → lossless strip (where tagged), plus §7.5 scenario tables. Add fixtures first, then implement until `cargo test -p wisp-invariants` passes.

## Claude Code

A one-page skill that teaches Claude Code to use wisp as shared memory is in `contrib/claude-skill/project-memory/SKILL.md`; copy it to `.claude/skills/project-memory/SKILL.md` in any repository that has a `.wisp/`. A CI workflow for fmt, clippy, tests, and release builds on three platforms is in `contrib/github-workflows/ci.yml`.

## License

**Community Release — GNU General Public License v2** (same copyleft family as [Git](https://git-scm.com/)). See [LICENSE](LICENSE) and [NOTICE](NOTICE).

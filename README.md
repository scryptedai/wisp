# wisp 0.1.0 — Community Release

A local-first, differential knowledge store for **Open Knowledge Format (OKF)** bundles.

This repository is **Scrypted’s GPLv2 Community Release** of wisp: source is here for use, study, modification, and redistribution under [LICENSE](LICENSE). Scrypted may offer other editions or licenses separately; **this tree is the community edition**.

Copyright © 2026 Scrypted. Author: Tim Cotten (tcotten@scrypted.ai).

wisp stores a bundle as an append-only log of **section-level** changes with encrypted, destroyable payloads, and materializes a conformant OKF directory on demand. It is built for **agent fleets on a shared bundle**: **optimistic, section-scope commits** (conflicts → exit 3), provenance on every change, and cryptographic erasure inside the store while the digest chain still verifies. **Ledger writes serialize on SQLite** (retryable lock contention → exit 7); managed checkout rewrites use `.wisp/worktree.lock`. See **[docs/CONCURRENCY.md](docs/CONCURRENCY.md)**.

Release **0.1.0** behavior is specified in **[docs/SEARCH.md](docs/SEARCH.md)** (retrieval), **[docs/ERASE.md](docs/ERASE.md)** (erasure and managed checkouts), **[docs/TRUST-BOUNDARY.md](docs/TRUST-BOUNDARY.md)** (what erasure and sync do not cover), **[docs/EXTENSIONS.md](docs/EXTENSIONS.md)** (optional product hooks), **[DECISIONS.md](DECISIONS.md)** (OKF/store deltas), and **[docs/MERGE.md](docs/MERGE.md)** (ref merge).

## Contributing

Fork, use **git worktrees** for parallel branches, and open PRs — see **[CONTRIBUTING.md](CONTRIBUTING.md)**. AI/review tools: **[AGENTS.md](AGENTS.md)**.

## Build

```sh
cargo build --release        # one static binary: target/release/wisp
cargo test                   # unit + engine invariant tests (§17)
cargo test -p wisp-invariants  # OKF corpus + search IR (semantic on by default; first run may fetch ~128MiB model)
```

**Air-gapped or offline CI:** build and test without neural search (no model download):

```sh
cargo build -p wisp-cli --no-default-features
cargo test -p wisp-invariants --no-default-features
```

For semantic tests with network once, set `FASTEMBED_CACHE_DIR` (CI uses a runner temp dir). Runtime: `wisp search … --local-only` or `WISP_LOCAL_ONLY=1` skips the neural tier ([docs/SEARCH.md](docs/SEARCH.md)).

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

A source turned out to be wrong? Ask what recorded work depends on it (read-only; historical mode):

```sh
wisp impact sources/payments-api.md               # dependents on the current branch, with the evidence chain
wisp impact sources/payments-api.md#01JAD2V7… --depth 5 --ref main --json
wisp impact 01JAD2V7000000000000000000            # an erased or removed source, by its concept id
```

Each result is a section or frontmatter version (`kind`) with its actor, timestamp, and `via` path back to the source: declared inputs (`pinned` to the snapshot the writer read, or `unversioned`), `inherited` from the version it replaced, or carried over by a `split`, `merge`, or `copy`. A write declared with `--inputs-complete` stops inheritance. Results are only as complete as the inputs writers declared, so an empty answer reads "No recorded dependents found within the selected scope." `wisp impact` never writes; opening a repository may complete a pending erase. Pin what you read with `--input <path>#<section>@<head from wisp cat --json>`.

Erase, and prove it (§10; see **[docs/ERASE.md](docs/ERASE.md)**):

```sh
wisp erase notes/standup.md#01JEF4X… --reason policy        # lists what it would destroy
wisp erase notes/standup.md#01JEF4X… --reason policy --yes  # key gone; hot tier purged; marker left
# unrecorded edits under a managed checkout block erase unless you pass --discard
wisp erase notes/standup.md#01JEF4X… --reason policy -y --discard
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

On disk (§15): `.wisp/manifest.toml`, `.wisp/ledger.db`, `.wisp/payloads/ab/<id>`, `.wisp/keys/keystore.db`, `.wisp/cache/refs/<ref-id>/hot.db` (rebuildable, excluded from backups), `.wisp/checkout.idx` (primary tree; linked worktrees use `.wisp-checkout.idx` in the checkout root).

## Keys

The keystore unlocks from, in order: `WISP_MASTER_KEY` (64 hex chars; CI), `--passphrase` (Argon2id, m=64 MiB, t=3, p=1), or `.wisp/keys/master.key` (created at `init`, mode 0600). `wisp backup --with-keys <dir>` writes the keystore to a separate destination; keep its retention short (§10.6).

## Status

| Milestone | State |
|---|---|
| M1 Parse (frontmatter, sectioning, anchors, identity) | done |
| M2 Cold tier and crypto (ledger, payload envelope, keystore, staging, changeset signing) | done |
| M3 Hot tier, optimistic commits, record, checkout, views, okf-0.2 | done |
| M4 Lifecycle and erase (retract/restore, groups, erase, fsck, policies, revert, backup) | done; managed checkout rewrite + open recovery — `docs/ERASE.md` |
| M5 Feed and search | feed, search (trigram/RI/semantic), pack, tokens (approx codec), external subscriber host done |
| M6 Profiles and migration | okf-0.1 profile, 0.1→0.2 migration plan, down-export (`export --profile 0.1`) done |
| M7 Concurrency and branches | headless writes, read assertions, branches, switch, worktrees, merge (fast-forward, disjoint replay, erase-wins, layered/nested replay, change-chain resolutions, `wisp mv` conflicts, `--resolutions`, `--abort`); merge replay skips durable in ledger — see `docs/MERGE.md`, `merge_playbook` + `merge_rules` |
| M8 Sync, git interop, release | `file://` + HTTP cold pack (`wisp serve --http <addr>`, Bearer `WISP_HTTP_TOKEN` — [docs/TRUST-BOUNDARY.md](docs/TRUST-BOUNDARY.md)) + SSH/rsync, `export --archive` tar.zst, `import --git` / `export --git` ([docs/GIT-INTEROP.md](docs/GIT-INTEROP.md)), `serve --stdio` ColdPack or local path ([docs/EXTENSIONS.md](docs/EXTENSIONS.md)) |

Agent desk demo (merge, erase, provenance): [`demo/desk/README.md`](demo/desk/README.md) · [`demo/README.md`](demo/README.md).

§17 invariant coverage (what is tested vs deferred): **[docs/INVARIANTS.md](docs/INVARIANTS.md)**. Integration: §11.5 merge (`merge_playbook`, `merge_rules`, `contrib/merge-rules/cases.toml`) and §11.6 sync (`sync_deletions`, `sync_roundtrip`) in `wisp-invariants`. Optional 50-writer stress: [contrib/concurrency/README.md](contrib/concurrency/README.md).

The **`wisp-corpus`** crate holds versioned OKF bundles (including a **SPY calls / news-signals** desk playbook under `bundles/spy-options/`), parse fixtures, and matcher scenarios (`catalog.toml`). **`wisp-invariants`** runs them on every CI build: record → validate → fsck → lossless strip (where tagged), plus §7.5 scenario tables. Add fixtures first, then implement until `cargo test -p wisp-invariants` passes.

## Claude Code

A one-page skill that teaches Claude Code to use wisp as shared memory is in `contrib/claude-skill/project-memory/SKILL.md`; copy it to `.claude/skills/project-memory/SKILL.md` in any repository that has a `.wisp/`. CI (fmt, clippy, tests, release build on three OSes): [`.github/workflows/ci.yml`](.github/workflows/ci.yml) (`contrib/github-workflows/ci.yml` is a fork copy).

## License

**Community Release — GNU General Public License v2** (same copyleft family as [Git](https://git-scm.com/)). See [LICENSE](LICENSE) and [NOTICE](NOTICE).

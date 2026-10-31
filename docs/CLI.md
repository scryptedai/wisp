# wisp CLI contract (agents and humans)

Installed topic docs live under a share directory (see `wisp doc path`). This file is the index: **what commands mean**, **what JSON does not prove**, and **stable exit codes**.

## JSON envelope

With `--json`, stdout is exactly one object:

```json
{ "ok": true, "command": "<name>", "data": { ... } }
```

On failure:

```json
{ "ok": false, "command": "<name>", "error": { "code": "<name>", "message": "...", "details": ... } }
```

Human text goes to stderr. Exit code matches `error.code` (numeric table below).

## Exit codes

| Code | Name | Typical cause |
|------|------|----------------|
| 0 | ok | Success |
| 1 | internal | Bug or I/O failure |
| 2 | usage | Bad flags or arguments |
| 3 | conflict | Stale `--base`, merge conflict (`section_conflict` vs `merge_conflict` in JSON) |
| 4 | refused | Policy / actor refusal |
| 5 | validation | Bundle or path validation |
| 6 | not_found | Missing ref, path, or version |
| 7 | write_timeout | Retryable SQLite lock (back off and retry); see [Exit 7 details](#exit-7-write_timeout-details) |
| 8 | integrity | Digest or fsck failure |
| 9 | key_unavailable | Keystore locked or missing key |
| 10 | erase_pending | Open must finish erasure recovery first |
| 11 | queue_timeout | Commit queue deadline (retryable; job not finished in time) |

See [CONCURRENCY.md](./CONCURRENCY.md) for writers and locks. Optional [commit queue](./COMMIT-QUEUE.md): `[queue] enabled = true` plus `wisp queue --serve`.

### Exit 7 (`write_timeout`) details

When `error.code` is `write_timeout`, `error.details` is an object agents can use to choose backoff vs batching vs branching (not only `retryable: true`):

| Field | Meaning |
|-------|---------|
| `retryable` | Always `true` for exit 7 |
| `lock` | Contended resource: `ledger`, `hot`, or `worktree_flock` |
| `phase` | Where the wait failed, e.g. `ledger.begin_immediate`, `commit.try_commit`, `worktree.flock_begin_immediate` |
| `busy_timeout_ms` | Manifest `commit.busy_timeout_ms` when known |
| `commit_attempt` / `commit_retries_max` | Present when engine retries are exhausted on `commit.try_commit` |
| `ref` | Ref name when the failure was on a commit |
| `sqlite` | SQLite result code name when the failure came from SQLite busy/locked |
| `hint` | Short remediation text |

Engine commit retries merge these fields into any ledger-level details from the last attempt.

## Target grammar

| Form | Meaning |
|------|---------|
| `path/to/note.md` | Concept by bundle path |
| `path/to/note.md#Heading` | Section by heading anchor |
| `path#01SECTION…` | Section by opaque section id |
| `01CONCEPT…` / `01SECTION…` | Opaque ids (after erasure or for agents) |
| `path#section@01CHANGE…` | Read assertion or pinned input (`head` from `wisp cat --json`) |

Unpinned `--input path#section` is stored **without** a version; wisp does not backfill the head seen at commit time.

## Two write paths

**`wisp record`** — Working-tree edits → one changeset → re-render OKF files. Use for human/agent edits on disk.

**Headless writes** — `update`, `add-section`, `remove-section`, `set`, `unset`, `create`, and per-op batches are **not** tied to the working tree. Each is shorthand for a one-op batch; see [APPLY.md](./APPLY.md). Prefer `wisp apply` for multi-op agent commits.

`--base` is the precondition version (`change` id, `absent`, or `any`). `--inputs-complete` declares the full dependency set (no inheritance); see [DECISIONS.md](./DECISIONS.md) for format marker interaction on 0.1.0.

## Commands vs topic docs

| Command | Topic doc | Read this for |
|---------|-----------|----------------|
| `init` | this page, [DECISIONS.md](../DECISIONS.md) §10.1 | Plain default; `--encrypt` for master key + ciphertext |
| `erase` | [ERASE.md](./ERASE.md) | Preview vs `--yes`, `--discard`, recovery, dependents |
| `merge` | [MERGE.md](./MERGE.md) | FF, replay, conflict exit 3, `--resolutions` |
| `search`, `pack`, `tokens`, `codec` | [SEARCH.md](./SEARCH.md), [EXTENSIONS.md](./EXTENSIONS.md) | Tier order, `--local-only`, token codec |
| `why` | [WHY.md](./WHY.md) | Backward graph, ref scope |
| `impact` | [IMPACT.md](./IMPACT.md) | Forward trace, historical-only `data.mode` |
| `export --html` | [EXPORT-HTML.md](./EXPORT-HTML.md) | Plaintext copy, publish semantics |
| `serve`, `push`, `pull` | [EXTENSIONS.md](./EXTENSIONS.md) | ColdPack / `stdio://`, not MCP |
| Trust / exports | [TRUST-BOUNDARY.md](./TRUST-BOUNDARY.md) | What erasure does not recall |

## What a successful answer still does not mean

- **Dependence is declared.** Empty `impact` means no recorded dependents in scope—not “nothing depends on this.” `no_declared_inputs` warns but does not block commits.
- **Erasure is scoped.** Keys and hot rows are destroyed; opaque ids and old plaintext exports remain linkable. See [TRUST-BOUNDARY.md](./TRUST-BOUNDARY.md).
- **Opening the repo has side effects.** Pending erase recovery and hot rebuild may run on open (including before `impact`).
- **`retract` / `remove-section` ≠ erase.** Retraction hides from the current view; erase is cryptographic destruction.

Repository agent guide: [AGENTS.md](./AGENTS.md) (copied beside topic docs on install).

## `wisp init` and cold storage

Default **`wisp init`** creates a **plain** repository: `manifest.toml` records `repo.storage = "plain"`, cold payload files store section text without AEAD (WSP1 version 2), scope keys sit in `keystore.db` as raw rows, and **no** `.wisp/keys/master.key` is created. `--passphrase` and `WISP_MASTER_KEY` are rejected at plain init (exit 2).

**`wisp init --encrypt`** selects encrypted cold storage for GDPR-style crypto-shred attestation: ChaCha20-Poly1305 payloads, a master key file (mode 0600), and wrapped scope keys. Encrypted repos unlock from, in order: `WISP_MASTER_KEY` (64 hex chars), `--passphrase` (Argon2id), or `.wisp/keys/master.key`.

Manifests **without** `repo.storage` are treated as **encrypted** so older repositories keep working. Trusted sync requires the same storage mode on both sides.

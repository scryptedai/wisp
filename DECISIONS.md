# DECISIONS.md

Deviations from the OKF store v1 design (ledger, parse, merge, sync, profiles), recorded with the section they affect. **Search and retrieval are specified in `docs/SEARCH.md`** — not listed here as deltas. Each entry states what the OKF design expects, what this build does, and why.

## §5 Data model

- **`reason` on `Change`.** The spec puts the retract/erase reason code only in the tombstone. This build adds an optional `reason` enum to every change (and a `reason TEXT` column on `changes`) so a retraction's reason code is in the ledger without free text. It is metadata (an enum), so it is covered by the digest (§10.3).
- **Frontmatter payloads.** The spec lists `frontmatter_key` and `frontmatter_value` as separate payload kinds. A change has one payload, so this build stores one `frontmatter_entry` payload `{key, raw}` under the concept scope. `Target::FrontmatterKey.key_commit` is still the keyed commitment to the key name under the concept's `K_com`.
- **Asset payloads** carry `u32 path_len | path | bytes` so `create_asset` is one payload (the spec names `asset_bytes` and `path` separately).
- **`SectionPayload`** stores the raw heading line(s) without the anchor in addition to the heading text, so setext headings, closing `#` sequences, and CRLF render back byte-for-byte (I3).

## §6.1 Ledger schema

- `target`, `inputs`, `valid`, `lineage`, `origin` are stored as JSON in the BLOB columns rather than canonical CBOR. The digest still uses canonical CBOR (RFC 8949 §4.2.1); the storage encoding is not covered by any integrity guarantee and JSON kept the decoder out of scope.
- One bookkeeping table was added: `erasure_markers (section, concept, after_section, reason)`, so a rebuilt hot tier can render an erased section's marker at its former position (§13.1).

## §6.2 Hot schema

- Additional columns/tables: `state_concepts.gen_actor / gen_at / content_version` (the inputs to the managed `generated` and `verified` keys, maintained on apply), `state_sections.heading_line`, `state_retracted`, `state_verified`, `state_erased_markers`. All are pure functions of the ledger (I13) and are checked by `hot check` through a logical dump comparison.
- `state_concepts.status` stores `unset` when `set_status` was never applied, so the renderer can omit the key (OKF reads absence as `stable`).

## §6.3 Reads and writes

- **Commit order inside the critical section.** The spec commits the ledger, then the hot tier. This build commits the hot tier first, then the ledger, both while holding the ledger write lock. Reason: with ledger-first, a writer acquiring the lock between the two commits sees a head whose changes are not yet in the tier and would wrongly conclude the tier is stale. If a process dies between the commits the tier is ahead of the ledger; `Repository::hot` sees an unknown head and rebuilds.
- **Rebuild holds the ledger write lock.** A rebuild computed from a stale head could otherwise be committed over a newer tier state.
- Built-in search indexes live in each ref’s `hot.db` under `cache/refs/` (`docs/SEARCH.md`). Token rollup DBs are not fully materialized; `pack` uses `tok:approx:chars4`.

## §7.2 Frontmatter CST

- Entries are split by a line-structured scanner (column-0 `key:` starts an entry; indented lines, `- ` items, multi-line flow collections and block scalars continue it; comments directly above an entry attach to it) instead of a `tree-sitter-yaml` CST. Values are parsed with `serde_yaml` only for comparison. Separator spans between entries are folded into the preceding entry's raw span on record, so untouched frontmatter round-trips byte-for-byte (I3) while the hot tier keeps one row per entry.
- A top level that is not a block mapping is stored as one opaque span and reported as non-conformant; this build does not yet rewrite it in block style on a managed-key write (the file is refused unless `--allow-nonconformant`).

## §7.4 Concept identity

- Rule 3 (no `wisp_id` = new concept) has one pragmatic exception: a file **without** a `wisp_id` at the path of a known concept, when no other file carries that concept's ID, is treated as that concept (reported as a repair). Without this, a model that strips the identity line produces a path conflict against the concept it was editing.

## §7.5 Section identity

- The preamble is identified by position: there is exactly one per concept, so an unanchored preamble reattaches to the prior preamble before rule 6 runs.
- Split (rule 6): the spec compares P's previous body to "P's new body followed by this section's heading line and body". This build accepts that form **or** the form without the heading line, since inserting a new heading into existing prose is the common split and the heading text never existed in the old body.
- Normalization trims trailing whitespace per line, normalizes line endings, and collapses trailing blank lines to one line break before comparison.

## §8 Operations

- `set_status` carries the status as a small encrypted payload (kind `status`) rather than in metadata.
- Editing `generated` or `verified` by hand is ignored silently (the spec says "a refusal otherwise"); `sources` and `status` edits map to `source_put`/`source_remove`/`set_status` as specified.
- Link rewriting on move is performed by `wisp mv` (as `update_section` changes in the same changeset). An OS rename recorded by `wisp record` produces `move` and an alias but does not rewrite inbound links.

## §9 Record pipeline

- Change detection records a SHA-256 of the bytes wisp last wrote in `checkout.idx`, so an mtime-only touch is not an edit and a concurrent rewrite is detected by content.
- After a commit, `record` re-renders every tracked file from the hot tier (step 10) rather than splicing anchors into the edited file. Byte-level losslessness is preserved by the lossless entry and section storage; the cost is proportional to the bundle size, which is acceptable for v1 and noted for optimization.
- A working-tree advisory lock (`.wisp/worktree.lock`, a SQLite `BEGIN IMMEDIATE`) serializes tree rewrites across processes.

## §10.1 Keys

- The default master-key source is a key file at `.wisp/keys/master.key` (mode 0600) instead of the OS keychain. `WISP_MASTER_KEY` and `--passphrase` (Argon2id with the specified parameters) work as specified. A keystore check value detects a wrong master key early (exit 9).
- Ed25519 signing: `wisp actor gen-key` generates a keypair, stores the seed at `.wisp/keys/signing/<actor>.seed` (or use `WISP_SIGNING_SEED` in CI), and registers the public key; `wisp actor key` still registers an external public key only. Commits sign the changeset digest when a matching local seed exists. `fsck` verifies signatures when the actor’s public key payload is present.

## §10.4 Erasure groups

- Group creation inserts the `groups` row directly (mutable bookkeeping) rather than through a change; membership goes through `group_add`/`group_remove` changes as specified.

## §10.5 Erase procedure

- The erase change is committed through the normal optimistic commit, and the tombstone, scope flags, payload-absence flags, and marker positions are written in a second ledger transaction immediately after. Both phases of shredding are idempotent and rerun on open, so an interruption between the two transactions is recovered.
- Phase 1 purges search-derived rows in hot.db (FTS, trigram, RI, dense embeddings) for erased sections, then hot tier state; token DB purge follows the same pattern when present.

## §11 Concurrency

- Linked worktrees (§11.4) are implemented minimally: `worktree add|list|remove`, checkout under `.wisp/worktrees/<name>/` (or `--path`) with `.wisp-checkout.idx`, ledger `worktrees` bookkeeping, and one ref per checkout. Merge (§11.5): fast-forward, divergent replay with a two-parent merge node, scope-key conflict detection (including same-commitment no-conflict, erase-wins with skip replay, retract-vs-content), layered changeset ordering for hot rebuild (`base` → `into` tip → `source` tip), `--resolutions` JSON (`take`: `ours`|`theirs` per conflict), `--abort` clears `.wisp/merge.json`; resolved merges and erase-wins persist skip lists under `cache/refs/<ref>/merge-skip.json` for hot rebuild/check. Concept path moves use `wisp mv` (not headless `move`). Duplicate paths are refused at commit (`PathFree`). Integration coverage: `merge_playbook`, `contrib/merge-rules/cases.toml`, and `merge_rules` in `wisp-invariants`. Sync (§11.6): `file://` push/pull, dumb HTTPS pack at `{remote}/wisp/v1/pack.zst`, SSH via `rsync` + local transport, and stdio Hello/Ack (`wisp serve --stdio`). See `docs/EXTENSIONS.md`. Branches, `switch`, headless writes, `apply` batches, read assertions, and `--assert-head` are.
- `--base any` is honored for headless operations; a policy rule with `unconditional = true` can deny it.

## §12 Profiles

- The okf-0.1 profile and the 0.1→0.2 migration plan (timestamp → `generated` with origin; `Citations` → sources) are implemented; down-export via `checkout --profile 0.1` / `export --profile 0.1` (to `--to` only on the primary worktree) is implemented.

## §13 Views and git interop

- Erasure markers for a section whose preceding section no longer exists are rendered after the preamble; for a concept with no sections left they follow the frontmatter.
- `export --archive` writes a `.tar.zst` of the rendered bundle. `export --git` renders and runs `git init` + one commit. `import --git` defaults to a **snapshot** (`git archive` + one `record`; `--squash` sets the message). `import --git --history` replays commits oldest-first (one wisp changeset per non-empty tree) and writes `meta/git-map.json` (git SHA → wisp changeset). `import <dir>` copies a plain bundle into the working tree and records it.

## §14–§16 Feed, search, CLI

- **Search:** `docs/SEARCH.md`. **Extensions:** `docs/EXTENSIONS.md`.
- **Search IR battery:** `contrib/search-battery/cases.toml` only.
- Erase targets an actor with `--actor-id <actor>` (the global `--actor` names the acting identity).
- `record --resolve` clears `.wisp/merge.json` after a successful record on the merge target ref; finish the merge with `wisp merge <source> <into> --resolutions <file>`.
- `wisp diff --from/--to` compares section-level ops along the `based_on` chain between two changesets; `diff` without those flags compares the working tree with its base.

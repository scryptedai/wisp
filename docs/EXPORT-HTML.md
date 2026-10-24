# HTML export (`wisp export --html`)

**Status:** Part 1 + Part 2 implemented on `feature/export-html` (overview/concept/section pages, indexes, pagination, word diffs, provenance SVG, `--since` / `--ids-only` / `--sign`, `export --verify`).

**Source:** Andy's *wisp export --html* design (Oct 2026), adapted to `main` after **impact** (#13): pinned inputs, `snapshot.rs`, ledger merge replay skips, `exports` table.

**Goal (Part 1):** Someone opens a **folder** in a browser (no wisp install, no network) and sees what wisp adds: stable section IDs, authorship, **declared** inputs, bounded history with diffs, merge-not-kept markers, and scoped erasure—without overstating provenance or integrity.

---

## Scope gate

| Ship in Part 1 | Park in Part 2 (do not scaffold) |
|----------------|----------------------------------|
| Overview, concept, section pages | Activity, actors, merges/removals indexes |
| Last **20** section versions + line diffs | Paginated history (100/page), `--since` |
| **≤12** direct inputs per version | Multi-hop provenance graph (SVG) |
| Merge skip markers from **ledger** `merge_replay_skips` | Merges index page |
| `--no-content`, `manifest.json`, `export-receipt.json` | `--ids-only`, signing, `export --verify` |
| Plaintext-copy notice on every page + CLI | Optional `site.js` filters |

**Stop rule:** Deliver Part 1, corpus/tests, documented limits, and a **5-viewer** smoke (4/5 answer author / inputs / erasure marker in 60s). Then stop. Part 2 needs explicit approval and a fresh estimate.

---

## CLI (Part 1)

```sh
wisp export --html <new-dir> [--ref <name>] [--no-content] [--since <changeset>] [--ids-only] [--sign] [--json]
wisp export --verify <existing-html-dir> [--json]
```

- Third export kind, **mutually exclusive** with `--git` and `--archive` (unchanged).
- `<new-dir>` **must not exist** (including empty). No `--force`, `--since`, `--at`, templates, or serve mode in Part 1.
- `--ref`: resolve one head; never switch checkout.
- `--json`: `{ok, command, data}` on stdout; warnings (including plaintext copy) inside envelope or stderr—never corrupt stdout.

**Publication safety**

- Resolve path against real parent; refuse repo, managed worktrees, ancestors, symlinks/reparse points.
- Render to **private staging**, then **no-replace rename** to final path (existence check + rename is not enough alone).
- **Register** export in `exports` **before** publish; failed registration → no publish; publish failure → conservative registry row remains.

**Success payload (illustrative):** `kind`, `dir`, `ref`, `head`, `content_mode`, `export_id`, page count, `manifest_sha256`, `warnings: ["plaintext_copy"]`.

---

## Site layout

```
site/
  index.html
  concepts/<ConceptId>.html   # nested <section id="…"> + revision history
  indexes/{activity,actors,merges,removals}.html
  assets/site.css
  manifest.json
  export-receipt.json
  README.txt
```

- Concept page filenames = validated opaque concept ids only (never user paths or titles).
- Section ids and change ids live in HTML `id` / `data-*` attributes (not shown as visible monospace in the mainline view).
- Works offline; semantic HTML; keyboard focus visible; optional inline script on index tables only.

**Pages**

1. **Overview** — ref, head, concept list, erasure/retraction markers, index links.
2. **Concept** — one page per concept: `<section id="{SectionId}">` per OKF section, current revision prominent, older revisions under `<details>`, diffs collapsed in `<details>`.
3. **Indexes** — activity (filterable), actors, merges (links to `#rev-{ChangeId}` on concept pages), removals.

**Do not use** `query::why` for export collection (wrong ref closure, `at` pin, concept-level dedup). Use a dedicated collector.

**Do not read** `.wisp/cache/.../merge-skip.json`; use `Ledger::merge_replay_skips` (same rule as merge hot rebuild).

**Impact alignment:** show `inputs_complete` when set on a change (`repo.format` ≥ 2). Reuse visual patterns from future `impact --report` only via shared CSS/components—not a hard dependency.

---

## Architecture

| Crate | Responsibility |
|-------|----------------|
| `wisp-engine` | **HtmlExportCollector**: one ref-head view, merge-aware reachable changesets, scope checks before decrypt, second redaction pass on model, merge skip decisions, input pins via `snapshot` helpers |
| `wisp-html` (new) | Render redacted **site model** → staging files only; no `Repository`, keys, or DB |
| `wisp-cli` | Parse flags, orchestrate collect → render → register → publish, user messages |

No subprocess `--json` pipeline for the snapshot (cannot guarantee one consistent authorized view).

### Consistent view (blocking spike)

**Phase 0 (1–2 days):** Which existing engine guard serializes with ledger commits, headless writes, and erase? A standalone worktree file lock is insufficient.

| Option | When |
|--------|------|
| **A — hold guard** (prefer Part 1) | Collect + render + register + publish while holding write guard; acceptable for demo-sized exports |
| **B — stamp + recheck** | Collect under guard, render unlocked, recheck stamp + erasure before publish; abort `repository_changed` |

If no guard meets the contract, add **narrow engine API** first and re-estimate.

**Collector prerequisites:** hot rebuilt if needed; refuse incomplete erasure / recovery; one transaction semantics for the view.

---

## Redaction and safe HTML

1. Check tombstones and scope erased/shredded **before** decrypt; filter model again before templates.
2. Erased content never in HTML, embedded JSON, SVG, filenames, manifest, receipt, or logs; opaque actor placeholders per store conventions.
3. Markdown: parser with raw HTML disabled/escaped; escape all template text and JSON-in-HTML; block `javascript:`, `data:`, `file:`, protocol-relative URLs, handlers, remote images.
4. Restrictive **CSP** verified under `file://` (Chromium + Firefox CI; manual Safari before claiming support).

`--no-content`: drop bodies, frontmatter values, diffs, messages, notes; keep IDs, times, input edges, merge markers. **Not anonymization** (headings/paths/actors may remain).

---

## Determinism

- Run id and wall time **only** in `export-receipt.json`.
- Sort collections and manifest paths deterministically.
- `manifest.json`: schema/renderer versions, flags, limits, SHA-256 of content files (exclude self + receipt).
- Same **redacted model + options + renderer** → identical content files; same head alone does **not** imply same bytes (erasure state changes). README states hash limits (no authorship proof).

---

## Delivery order

Shipped on **0.1.x** (see `CHANGELOG.md` and `contrib/export-html/README.md`):

1. `wisp-html` crate (model v3, semantic concept pages, indexes, manifest/receipt).
2. `wisp export --html` with worktree lock, hot-tier collector, scoped redaction, staging beside destination (same filesystem as publish target).
3. Export registry row written **after** successful publish (inside an immediate transaction with a head/erasure recheck) so failed publishes do not leave `exports` rows.
4. `wisp-invariants` acceptance tests (`export_html.rs`); demo via `impact-demo` / desk workspace scripts.

Part 2 flags (`--since`, `--ids-only`, `--no-content`, `--sign`, `export --verify`, activity/actors/merges/removals indexes) ship in the same release train as Part 1.

---

## Acceptance (Part 1)

| Check | Criterion |
|-------|-----------|
| View | Concurrent write/erase → clean abort or one valid view—never mixed head/labels |
| Ref scope | No unrelated branch history; pins resolve or show explicit unresolved |
| Merge markers | From ledger replay; survive cache delete; never read merge-skip.json |
| Diffs | Vs replay-visible predecessor; missing baseline stated |
| Erasure | Per-scope canaries absent from **every** output byte |
| `--no-content` | No bodies/values/diffs/messages anywhere |
| Offline safety | Hostile markdown inert; relative links resolve; no auto network |
| Publish | Existing dirs untouched; registry after atomic publish; `fsck` still passes |

---

## Part 2 (shipped)

`--since`, `--ids-only`, activity/actors/merges/removals indexes, provenance graph (bounded depth/nodes), word-level diffs, section-history pagination, optional `--sign` / `export --verify`, and export registry fields (destination, manifest hash, status). See `DECISIONS.md` for locking (worktree lock), renderer (`wisp-html` string templates), and manifest schema `wisp-html-export/3`.

---

## Related docs

- [contrib/export-html/README.md](../contrib/export-html/README.md) — smoke checklist and demo commands
- [IMPACT.md](IMPACT.md) — forward tracing (complementary to export’s backward input display)
- [ERASE.md](ERASE.md) — scoped redaction and recovery
- [MERGE.md](MERGE.md) — replay skips in ledger
- [TRUST-BOUNDARY.md](TRUST-BOUNDARY.md) — plaintext copy and export limits

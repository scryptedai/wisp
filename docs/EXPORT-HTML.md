# HTML export (`wisp export --html`) — Part 1 (shipped)

**Status:** Part 1 implemented in Community Release 0.1.x; Part 2 is reference only until scope and estimate are re-approved.

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
wisp export --html <new-dir> [--ref <name>] [--no-content] [--json]
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

## Site layout (Part 1)

```
site/
  index.html
  concepts/<ConceptId>.html
  sections/<SectionId>.html
  assets/site.css
  manifest.json
  export-receipt.json
  README.txt
```

- Filenames = validated opaque IDs only (never user paths or titles).
- No copied bundle assets, remote scripts, or client JS in Part 1.
- Works offline; semantic HTML; system fonts; keyboard focus visible.

**Pages**

1. **Overview** — ref, head, live concepts/sections, removal markers, one demo route (use **`impact-demo`** corpus narrative or a thin `export-demo` spin-off).
2. **Concept** — current markdown + frontmatter; section links; deprecation/retraction/erasure badges per store rules.
3. **Section** — selected version metadata; ≤12 direct inputs (pinned / unversioned / unresolved / erased / retracted / none); ≤20 versions newest-first with **line diff** vs **replay-visible predecessor** (`similar` or equivalent); `"Not kept at merge <ChangesetId>"` from ledger replay only; older versions count + limit notice; bodies in `<details>` when content on.

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

```text
0. Locking spike → document chosen option (DECISIONS.md if new API)
1. Redaction canary fixtures + collector skeleton (no HTML yet)
2. wisp-html crate + golden tests on redacted model
3. Three page types + manifest/receipt/README
4. CLI wire-up + export registration + destination races (incl. Windows)
5. Corpus: extend impact-demo or add export-demo; wisp-invariants acceptance tests
6. Browser smoke + 5-viewer script; update README status + CHANGELOG
```

**Estimate:** ~1–2 focused weeks including spike, review, and tests (Andy doc). Re-estimate after phase 0 if new engine locking is required.

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
| Publish | Existing dirs untouched; register-before-publish; `fsck` still passes |

---

## Part 2 reference (not approved)

When approved separately: `--since`, `--ids-only`, activity/actors/merges/removals pages, provenance graph (depth/node bounds), word-level diffs, pagination, optional signing/`--verify`, richer export registry (destination, manifest hash, status). Budget **+2–4 weeks** before optional modules. **First Part 2 candidate:** merges index (completes the “two agents collided” story Part 1 only hints via section markers).

---

## Decisions to record in `DECISIONS.md` when implementing

- Locking option A vs B (and any new guard API).
- Template engine choice (`askama` vs `minijinja`) after dependency review.
- Whether Part 1 demo reuses `impact-demo` only or adds `export-demo` for HTML-specific canaries.
- Exact `manifest.json` schema version and CSP directive string per browser matrix.

---

## Related docs

- [contrib/export-html/README.md](../contrib/export-html/README.md) — smoke checklist and demo commands
- [IMPACT.md](IMPACT.md) — forward tracing (complementary to export’s backward input display)
- [ERASE.md](ERASE.md) — scoped redaction and recovery
- [MERGE.md](MERGE.md) — replay skips in ledger
- [TRUST-BOUNDARY.md](TRUST-BOUNDARY.md) — plaintext copy and export limits

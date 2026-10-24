# Agent writers (headless fleets)

Contract for **local or co-located agents** committing to one wisp bundle. Human disk editing uses `wisp record` ([`docs/WORKTREE.md`](WORKTREE.md)); agents should prefer **headless** / `wisp apply` and **hot** reads.

## Source of truth

| Need | Use |
|------|-----|
| Current section body, versions, refs | **Hot tier** — `wisp cat`, `wisp apply` read path, engine APIs |
| Search, pack, impact, why | Engine query on hot + ledger |
| Files on disk under the bundle root | **Cache** — may lag after headless commits; see [Materialize](#materialize-disk-cache) |

Do not treat the working tree as authoritative immediately after `apply` unless you barrier (below).

## Two write modes

### 1. Canonical replace — `update_section`

- Sends the **full** new section body.
- Carries **`base_version`** (section change id) unless you explicitly pass `"any"`.
- Another writer advanced the same section → **exit 3** (`section_conflict`), with `theirs.version` in JSON.
- **Default for:** contracts, edits, anything that must not silently lose peer text.

**On exit 3 (required client behavior):**

1. Do **not** crash the fleet worker; treat as normal flow.
2. Read `details[0].theirs.version` (or refresh section version from hot / `wisp cat --json`).
3. Rebuild your op with that `base_version` (or abort if policy says escalate to human).
4. Retry with backoff+jitter if many agents collide.

**Built-in loop:** `wisp apply --resolve` or `wisp_engine::headless::apply_with_resolve` (policy: `ApplyResolvePolicy`, default attempt budget scales with `commit.retries`). Embeds should use the engine API instead of reimplementing the loop; custom merge logic can still call `apply` once and handle conflicts itself.

Exit 3 is **per request**, not per process. A “poison” client that never refreshes only fails itself; see adversarial battery **D2-poison-neighbor**.

### 2. Logging patch — `append_section` / `prepend_section`

- Sends a **fragment** only; engine stitches onto the hot body (newline-normalized).
- **No** section-version precondition — no exit 3 for stale base on that op.
- Concurrent appends serialize on the ledger; order = commit order.
- **Default for:** metrics, traces, agent narration, multi-writer log lines on one section.

Full replace still uses `update_section`. Mixing blind append with full replace on the same section is valid: append does not block stale **update** (update still exit 3 when base is wrong). See `docs/APPLY.md`.

## Exit codes (agents care about these)

| Code | Meaning | Action |
|------|---------|--------|
| **3** | Optimistic conflict (section, FM, path, …) | Refresh base, retry or merge policy |
| **7** | SQLite busy (retryable) | Backoff; manifest `commit.retries` also applies in-engine |
| 4 | Refused (policy / generated file) | Fix op or use `record --adopt-generated` for humans |
| 0 | Success | — |

**Not exit 3:** `wisp merge` **scope_conflict** on divergent refs — resolve with `wisp merge --resolutions` or branches. See [`docs/MERGE.md`](MERGE.md).

## Throughput patterns

- **One section, many loggers** → `append_section` (or one section per agent).
- **One section, one editor** → `update_section` + refresh on 3.
- **Many sections, one ref** → disjoint sections; parallel in principle, **SQLite serializes** commits (expect exit 7 under load).
- **Parallel features** → `wisp branch` / `wisp worktree`, merge when ready ([`battery-error-three`](../contrib/battery-error-three/README.md) triple-lane pattern).
- **Burst writers** → batch `wisp apply` when ops are independent; avoid huge single batches with stale bases.

Manifest tuning: `[commit] busy_timeout_ms`, `retries`; `[materialize] enabled`, `tick_ms` — `wisp config get materialize.tick_ms`.

## Materialize (disk cache)

After headless commits, publish work is **async** (heartbeat `materialize.tick_ms`, default 250 ms). Barriers:

- `wisp materialize --wait` / `Repository::flush_materialize()`
- `wisp record` **drains** before return (human path)
- `wisp push` / cold pack — flush materialize + cold payloads
- `Repository::drop`

Poll lag: `wisp status --json` → `materialize.pending_jobs`, `materialize.lag_ms`.

Tools that only read paths must barrier or read hot. See [`docs/WORKTREE.md`](WORKTREE.md).

## Actor strings

Use stable actor ids: `human:<id>`, `process:<id>`, or `<producer>/<version>`. First commit registers the actor. Declare **`inputs`** on content derived from other concepts (`wisp why`, `wisp impact`); optional `inputs_complete` per `docs/APPLY.md`.

## Validation batteries (CI)

Run before changing writer behavior:

| Battery | Command | Proves |
|---------|---------|--------|
| **error-three** | `cargo test -p wisp-invariants battery_error_three_section_one -- --nocapture` | Timing + solo / dual exit 3 / triple merge |
| **exit3-adversarial** | `cargo test -p wisp-invariants battery_exit3_adversarial -- --nocapture` | Stampede, resolve, disjoint control, poison neighbor |
| **exit7-adversarial** | `cargo test -p wisp-invariants battery_exit7_adversarial -- --nocapture` | Write-queue saturation, client vs engine retry, batch compare, commit profile |

Edit cases only in `contrib/battery-error-three/cases.toml`, `contrib/battery-exit3-adversarial/cases.toml`, and `contrib/battery-exit7-adversarial/cases.toml`.

Heavy scale (optional):

```sh
WISP_EXIT3_BATTERY=1 WISP_EXIT3_BATTERY_WRITERS=32 \
  cargo test -p wisp-invariants battery_exit3_adversarial_heavy -- --ignored --nocapture
WISP_EXIT7_BATTERY=1 WISP_EXIT7_BATTERY_WRITERS=16 \
  cargo test -p wisp-invariants battery_exit7_adversarial_heavy -- --ignored --nocapture
WISP_STRESS=1 WISP_STRESS_WRITERS=50 \
  cargo test -p wisp-invariants -- --ignored stress_many_disjoint_writers
```

## Sync / remotes

`wisp push` / `wisp pull` move cold packs; trust remotes per [`docs/TRUST-BOUNDARY.md`](TRUST-BOUNDARY.md). Sync does not change exit-3 rules on the local bundle after pull — replay and hot rebuild apply upstream changes; writers still use fresh bases on shared sections.

## Related

- [`docs/APPLY.md`](APPLY.md) — batch JSON, append/prepend examples
- [`docs/CONCURRENCY.md`](CONCURRENCY.md) — guarantees and limits
- [`docs/MERGE.md`](MERGE.md) — ref merge vs section exit 3
- [`DECISIONS.md`](../DECISIONS.md) — async materialize, patch ops

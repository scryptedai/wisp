# Federation (composite knowledge bases)

Design sketch for **many OKF bundles** (A, B, C, …) composed into one working store — “GitHub for agents” without treating a plain Git clone as the source of truth.

## Goals

| Goal | Mechanism in wisp today | Gaps |
|------|-------------------------|------|
| Pin upstream content | `wisp pull` / cold packs, ref heads | No first-class **vendor ref** naming in corpus |
| Integrate upstream edits | `wisp merge`, branch/worktree | No automated “vendor bump” workflow |
| Local behaviors / overlays | Headless writes on `main` or feature branches | No package manifest for “which bundles” |
| Trust | [`docs/TRUST-BOUNDARY.md`](TRUST-BOUNDARY.md) | Registry / signing policy for remotes |
| Agent writes | [`docs/AGENT-WRITERS.md`](AGENT-WRITERS.md) | Cross-bundle `inputs` at scale |

## Recommended layout (conceptual)

```text
my-hub/                 # your bundle (ledger is authoritative)
  vendor/
    payments/           # linked checkout or subtree from remote pack
  behaviors/
    retry-policy.md     # local sections + inputs pointing at vendor paths
  main                  # ref agents commit to after merge
```

- **Vendor** content is **read-mostly**: pull to a pinned changeset, merge into `main` when you accept upstream.
- **Behaviors** are **yours**: declare `inputs` to vendor sections/concepts so `wisp why` / `wisp impact` stay honest.
- **Do not** overwrite `.wisp/` or hot state from a Git-only mirror; use `wisp push` / `wisp pull` (ColdPack + ledger replay).

## Hub vs leaf

| Role | Holds | Agents |
|------|--------|--------|
| **Leaf** bundle | One product KB | Write with headless; branch per task |
| **Hub** bundle | Vendored refs + local behaviors | Merge vendor bumps; fleet writes on disjoint sections or branches |

A hub may also **publish** a feed (`wisp feed`, ColdPack) for downstream leaves — same trust rules as push/pull.

## Operations map

1. **Add vendor** — register remote, `pull` into a branch, `merge` to `main` (or keep long-lived `vendor/foo` ref).
2. **Bump vendor** — pull newer pack, diff (`wisp diff`), merge or conflict-resolve (`wisp merge --resolutions`).
3. **Agent task** — `wisp worktree` or branch; `apply` / `apply_with_resolve`; merge when CI batteries pass.
4. **Search** — local index per bundle ([`docs/SEARCH.md`](SEARCH.md)); federated search across bundles is **not** in 0.1.0 (would need a meta-index or query fan-out).

## Not in scope (yet)

- Global package registry with semver resolution
- Automatic transitive vendor closure
- Cross-repo atomic commits
- Replacing Git hosting — Git export/import remains [`docs/GIT-INTEROP.md`](GIT-INTEROP.md)

## Related

- [`docs/MERGE.md`](MERGE.md) — ref merge vs section exit 3
- [`docs/WORKTREE.md`](WORKTREE.md) — linked checkouts under `.wisp/worktrees/`
- [`DECISIONS.md`](../DECISIONS.md) — store behavior changes

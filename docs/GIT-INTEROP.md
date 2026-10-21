# Git interop and anchor safety (0.1.0)

Wisp identity is **section anchors** and **concept `wisp_id` lines** in the OKF bundle. Plain Git repositories store markdown without that metadata. This guide describes safe workflows and known limits.

## What works well

| Goal | Workflow |
|------|----------|
| Publish a snapshot | `wisp export --git <dir>` — one commit of the **rendered** tree at the current ref |
| Bootstrap from Git | `wisp import --git <repo>` (snapshot) or `--history` (one changeset per commit, oldest first) |
| Keep wisp as source of truth | Edit only through **managed checkouts**, `wisp record` after every change, use Git export as **read-only** handoff |

## What breaks anchor identity

- **Hand-editing** exported markdown in Git without wisp (anchors stripped or headings changed).
- **`git merge`** on exported repos — conflict resolution rewrites sections; anchors do not round-trip.
- **Re-import** after external edits — `wisp-match` heuristics ([DECISIONS.md](../DECISIONS.md) §7.5) may attach the wrong section or mint new IDs.

Ref merge (`wisp merge`) compares **ledger scope keys**, not the full §7.5 matcher table. See [docs/MERGE.md](MERGE.md).

## Recommended patterns

1. **Single writer of record** — one wisp bundle (or branch/worktree per agent); Git is downstream.
2. **Record before export** — `wisp record` so the ledger matches the tree you export.
3. **Do not `import --git` into a live bundle** without a backup; treat import as initial seed or a controlled migration.
4. **Agents** — prefer headless `update_section` / `record` over editing exported Git clones.

## Erasure and Git

`export --git` copies **plaintext** at export time. Erasing inside wisp does not remove commits already pushed to a remote Git host. See [docs/TRUST-BOUNDARY.md](TRUST-BOUNDARY.md).

## Related commands

| Command | Role |
|---------|------|
| `wisp export --archive out.tar.zst` | Rendered bundle archive (no Git) |
| `wisp import <dir>` | Copy a plain directory and record |
| `meta/git-map.json` | Written by `import --git --history` (git SHA → wisp changeset) |

Extension transports: [docs/EXTENSIONS.md](EXTENSIONS.md).

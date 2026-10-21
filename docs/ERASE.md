# Erasure and managed working copies (§10.4–10.6)

This document matches `wisp erase`, `crates/wisp-engine/src/erase.rs`, and recovery on `Repository::open`.

## What gets erased

- The **crypto-erasure unit** is a **scope** (typically one section, concept, asset, or actor identity), not an arbitrary sentence inside a section.
- **Cold tier**: erase changeset, tombstone, scope flags, payload absence, optional erasure markers; Phase 2 shreds ciphertext and **scope keys**.
- **Hot tier**: section/concept state, search indexes, vectors for affected sections.

Copies in **other scopes**, **external exports**, **backups without `.wisp/`**, or **plaintext outside Wisp-managed paths** are out of scope unless you erase those scopes too or destroy the copies yourself. See also **[docs/TRUST-BOUNDARY.md](TRUST-BOUNDARY.md)**.

## Managed working copies (no whole-disk scan)

Wisp only rewrites checkouts it knows about:

| Checkout | Path source |
|----------|-------------|
| Primary worktree | `repository.root` (checked-out ref in `.wisp/checkout.idx`) |
| Linked worktrees | `worktrees` ledger table (`wisp worktree add`) |

After shred, **`rewrite_all_managed_worktrees`** re-renders every row in that table plus the primary root. **`wisp erase`** checks all of them for unrecorded edits **before** committing the erase; pass **`--discard`** to overwrite dirty managed files. **`recover`** on `Repository::open` rewrites with discard enabled when **`erase_rewrite_pending`** is set or when shred was interrupted.

Erase returns an error unless **`assert_erase_complete`**: every affected scope is **shredded** and none are **erased-but-not-shredded**.

## Recovery boundaries

On every `Repository::open`, **`erase::recover`** runs before other work:

1. **`finish_erase_bookkeeping`** — If an **`Op::Erase`** change exists in the ledger **without** a matching tombstone row (crash after commit, before the bookkeeping transaction), tombstone, scope flags, markers, and pending subscriber rows are written idempotently.
2. **`shred`** — For scopes with `erased = 1` and `shredded = 0` (crash after bookkeeping, before or during shred). Hot purge deletes rows by scope id **and** removes section ids recorded on the ledger scope row (so a stale `state_sections.scope` column cannot leave ciphertext in hot).
3. **`rewrite_all_managed_worktrees`** — Re-render from hot, then **`prune_tracked_files_removed_from_index`** so concepts removed from hot disappear from disk even when the prior `checkout.idx` is still needed for `remove_stale` inside `write_tree`.

A crash **after** shred but **before** worktree rewrite leaves **`erase_rewrite_pending`** set; the next **`open`** runs **`recover`** and rewrites all managed checkouts (no whole-disk scan).

## Sync and replicas

Trusted `file://` sync mirrors erasures on the destination after hot rebuild; see `DECISIONS.md` §11 and `crates/wisp-invariants/tests/sync_deletions.rs`.

## Tests

| Area | Where |
|------|--------|
| Canaries, reopen | `crates/wisp-engine/src/tests.rs` (`i4_i5_i21_i22_erase_section_canaries_and_integrity`) |
| Linked worktree | `erase_rewrites_linked_worktree_for_ref` |
| Stale managed file | `rewrite_managed_worktrees_clears_stale_plaintext_after_erase` |
| Commit-before-bookkeeping crash | `recover_after_erase_commit_before_bookkeeping` (targets the **Alpha** section scope, not the preamble) |
| Replica erase + keys | `crates/wisp-invariants/tests/sync_deletions.rs` |

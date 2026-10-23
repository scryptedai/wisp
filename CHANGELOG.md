# Changelog

User-visible changes to the wisp Community Release. Spec deviations and their reasons are in [DECISIONS.md](DECISIONS.md).

## Unreleased

### Added

- `wisp export --html <new-dir> [--ref <name>] [--no-content] [--json]`: static, offline HTML export (overview, concept, and section pages; bounded history with line diffs; merge-not-kept markers from ledger replay skips; plaintext-copy notice). See [docs/EXPORT-HTML.md](docs/EXPORT-HTML.md).
- `wisp impact <path|concept-id>[#section] [--depth N] [--ref R]`: read-only, historical-mode forward tracing from a source to the recorded work that depends on it, with the evidence chain for each result. Works for retracted sources by path and for erased or removed sources by concept id.
- `--input <path>#<section>@<changeset>` pins the snapshot an input was read at (`head` from `wisp cat --json`). Pins are validated.
- `--inputs-complete` (and `inputs_complete` in `wisp apply` batches) declares that a change's inputs are its complete dependency set; otherwise a new section version inherits the dependencies of the version it replaced.
- `wisp apply` ops accept their own `inputs`, overriding the batch-level inputs for that op. Batch format: [docs/APPLY.md](docs/APPLY.md).
- `wisp erase` (preview and result) reports recorded dependents on every ref, grouped by ref (`dependents_by_ref`); `flagged` is their union.
- `wisp retract` lists recorded direct dependents of the retracted concept on the ref (`flagged`, the same shape as `wisp erase`), read-only.
- Writes by non-human actors that add or change section content without declaring inputs return a `no_declared_inputs` warning (never a refusal).

### Changed

- **`wisp why` is scoped to the selected ref** (default: the ref checked out in the working tree; `--ref` to override). It used to list every change to the section in the ledger, including changes on unrelated branches. Its JSON now includes `ref`.
- `--input` with a malformed section id is a usage error (exit 2) instead of silently recording a whole-concept input.
- `at` in `wisp apply` inputs is validated: the changeset must exist and the section must exist at it.

### Compatibility

- Ledgers gain a nullable `changes.inputs_complete` column, added automatically on open.
- The first use of `--inputs-complete` raises `manifest.toml` `repo.format` to `2` (repositories that never use it stay at `1`). From this release on, wisp refuses a format newer than it knows with "this repository uses a newer format; upgrade wisp". wisp 0.1.0 does not check the format: on a format-2 repository its `fsck` reports digest mismatches for changesets that use `--inputs-complete`, so upgrade every reader before using the flag.

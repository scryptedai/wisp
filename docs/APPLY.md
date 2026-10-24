# `wisp apply` batch format

`wisp apply [<file>]` reads one JSON batch (from the file or stdin) and commits it as one changeset. Every headless command (`update`, `add-section`, `set`, …) is shorthand for a one-op batch. See §11.3 for the precondition rules.

```json
{
  "ref": "main",
  "actor": "claude-code/1",
  "message": "Fold the payments doc into the retry policy",
  "base": null,
  "assert_head": null,
  "inputs": [
    { "concept": "sources/payments-api.md", "section": "01JAD2V7…", "at": "01JA9Z0T…" }
  ],
  "assert": [ { "path": "sources/payments-api.md", "section": "01JAD2V7…", "version": "01JB4K2Q…" } ],
  "ops": [
    { "op": "update_section", "path": "notes/retry-policy.md", "section": "01JB…",
      "base_version": "01JC…", "content": "Retry 5 times with backoff.\n" },
    { "op": "update_section", "path": "notes/auth-flow.md", "section": "01JD…",
      "base_version": "01JE…", "content": "Refresh tokens daily.\n",
      "inputs": [ { "concept": "sources/auth-spec.md", "section": "01JF…", "at": "01JA9Z0T…" } ] }
  ]
}
```

## Logging patches (`append_section` / `prepend_section`)

For multi-agent **log lines** on an existing section, use delta ops instead of replacing the whole body:

```json
{ "op": "append_section", "path": "notes/swarm-metrics.md", "section": "01J…",
  "content": "Explorer-1: queue depth 3.\n" }
```

- No `base_version` check (no exit 3 for a stale section version).
- Payload is a **fragment** stitched onto the current hot body (newline-normalized).
- Ref merge replays patch-only chains on the same section without a `scope_conflict` when both sides only used append/prepend since the base.

Full **replace** edits remain `update_section` with `base_version` (default optimistic concurrency).

## Refresh on conflict (`--resolve`)

`wisp apply --resolve` (and `wisp_engine::headless::apply_with_resolve`) retries the same batch when commit returns **exit 3** for `section_conflict`: each affected op’s `base_version` is set from `theirs.version` (or refreshed from hot), then apply runs again until success or the attempt budget is exhausted. The JSON result includes a `resolve` object with `attempts`, `conflict_rounds`, and `retryable_errors`. Exit **7** after in-engine `commit.retries` is also retried at this layer.

Only **section_conflict** is auto-patched; other conflict kinds (e.g. `section_removed`, `path_conflict`) fail immediately. Fleet contract: [`docs/AGENT-WRITERS.md`](AGENT-WRITERS.md).

## Inputs

An input names what a change was derived from: `concept` is a path (or concept id), `section` optionally narrows it to one section, and `at` optionally pins the **snapshot that was read** (a `ChangesetId`, such as `head` from `wisp cat --json`; not a section version). A pinned input is validated before commit: the changeset must exist and the section must exist at that snapshot. Without `at` the input is recorded unversioned; wisp never infers the snapshot from the head at commit time.

- **Batch-level `inputs`** apply to every content change in the batch unless an op sets its own `inputs`.
- **Per-op `inputs`** replace the batch-level inputs for that op's content changes. An explicit empty list (`"inputs": []`) declares that op has no inputs.

- **`inputs_complete`** (batch-level boolean, or per-op to override it) declares that the inputs a change receives are its **complete** dependency set. Without it, a new version of a section inherits the dependencies of the version it replaced, however different the new content is, whoever writes it, and however many inputs it lists. `"inputs_complete": true` with no inputs declares an original observation with no sources. The CLI flag is `--inputs-complete`.

Inputs attach to content changes only (`create`, sections, frontmatter, sources). They are what `wisp why` walks backward and `wisp impact` walks forward.

## Warnings

When an actor that is not `human:*` writes section content with no declared inputs, the result carries `warnings: [{ "code": "no_declared_inputs", ... }]`. It is advisory and never blocks the write.

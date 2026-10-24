---
name: project-memory
description: Use wisp as the shared, versioned memory for this repository whenever you read or change design docs, decision logs, or notes under the OKF bundle (any directory with a .wisp/). Covers hot vs disk, base versions, exit 3 and wisp apply --resolve, append_section for logs, materialize barriers, packing context, and erasing leaked secrets.
---

# Project memory with wisp

This repository keeps its knowledge in an OKF bundle managed by `wisp` (one binary; every command takes `--json`). Other agents and people write to the same bundle at the same time. Follow these steps instead of editing the markdown files directly.

**Authority:** hot tier and `wisp cat` — not necessarily the files on disk immediately after headless writes. Run `wisp materialize --wait` before reading paths from disk, or use `wisp cat` / `wisp search`.

## Before you write code

1. Find what is already known:
   `wisp search "<topic>" --json` → sections with token counts.
2. Load exactly what you need, within budget:
   `wisp pack <path>… --budget 4000 --json` → `concepts[].sections[]` with `text`; `omitted` lists what did not fit.
3. If a decision looks surprising, ask why before changing it:
   `wisp why <path>#<section> --depth 2 --json` → actors, timestamps, and the inputs each change was derived from.

## Changing a design decision or a note

1. Get the base version of the section you will change:
   `wisp cat <path>#<section> --json` → `data.version`, `data.content`, `data.head`.
2. Commit against that base. Name what you derived it from, pinned to the `data.head` you read it at, and say why:
   ```
   wisp update <path>#<section> --base <version> \
        --content "<new text>" \
        --input <observation-path>#<section>@<head> \
        -m "<why this changed>" --json
   ```
   `--input` takes the snapshot (`data.head` from `wisp cat`), not the section's `data.version`. Repeat `--input` for every source you used. If you rewrote the section entirely from the inputs you list, add `--inputs-complete`; otherwise the new version keeps the dependencies of the one it replaced. A result with `warnings[].code == "no_declared_inputs"` means you declared nothing: fix it on your next write unless the content really is your own observation (then pass `--inputs-complete` with no `--input`).
   Use `wisp add-section <path> --after <section|^> --heading "<h>" --content "…"` for new sections and `wisp set <path> <key> '<json>' --base absent` for frontmatter.
3. Edits to different sections do not conflict, so do not serialize work with other agents on disjoint sections.

## Multi-agent log lines on one section

Use a JSON batch with `append_section` (fragment only; no `base_version`; no exit 3 for stale section version):

```json
{ "actor": "process:you", "message": "metric", "ops": [
  { "op": "append_section", "path": "notes/log.md", "section": "<id>", "content": "line\n" }
]}
```

`wisp apply -` or `wisp apply batch.json --json`. Full replacements still use `update_section` / `wisp update` with `--base`.

## When the exit code is 3

The result has `error.details.conflicts[]`. Each object carries `base.content` (what you read), `theirs.content` with `theirs.actor` and `theirs.version` (what landed meanwhile), and `yours.content`. Write a resolution that keeps both intents, then resubmit with `--base <theirs.version>`, or use `wisp apply --resolve` on the same JSON batch so wisp refreshes `base_version` and retries. Never treat exit 3 as fatal for the whole fleet worker.

## When a source turns out to be wrong

Find the recorded work that depends on it before you change anything:
`wisp impact <path>[#section] --json` → `data.results[]`, each with `path`, `section_id`, `version`, `actor`, `at`, and `via` (the evidence chain back to the source). Review each result; do not repair automatically. `data.truncated: true` means raise `--depth`. An empty result says "No recorded dependents found within the selected scope." — it is not proof that nothing was affected, because inputs are only what writers declared. A retracted or erased source works too (use its concept id once erased).

## Identity

Set `WISP_ACTOR` to `process:<your-name>` or `<tool>/<version>`. Never commit as `human:*`. Policies may refuse some operations to processes (exit 4, `error.details.rule`); report the refusal instead of working around it.

## Secrets and personal data

If you notice a credential or personal data in the bundle, do not paraphrase it. Erase the section and note the erasure in your summary:
`wisp erase <path>#<section> --reason policy --yes --json`
`wisp fsck` must still pass afterwards; include its one-line result.

## Do not

- Edit `index.md` or `log.md`; they are generated.
- Remove `wisp_id:` lines or the `<!-- ^… -->` anchors from headings; they are the identity of what you are editing.
- Assume headless commits updated files on disk; call `wisp materialize --wait` or read hot via `wisp cat`.
- Run `wisp record` on paths you did not edit on disk unless you are adopting human edits (`record` drains materialize and commits working-tree changes).

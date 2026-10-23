# Impact demo: a withdrawn payments doc

Fixture: `bundles/impact-demo/`. Test: `wisp-invariants/tests/impact.rs`.

The September payments API doc turns out to be wrong (unbounded retries, unsigned webhooks). Agents wrote notes from it. The question is: **which recorded work depends on it?**

| Note | Recorded inputs | `wisp impact sources/payments-api-2026-09.md` | Why |
|---|---|---|---|
| Retry policy | September doc `#Retries`, pinned | Appears (depth 1, `pinned`) | Direct input |
| Error handling guide | Retry policy, pinned at the version written from the September doc | Appears (depth 2) | Two-step chain through an affected version |
| Webhook setup | September doc `#Webhooks` (v1), then rewritten from the October doc with `--inputs-complete` (v2), then a typo fix (v3) | v1 only | Historical: v1 recorded the old doc. v2 declared a complete replacement, so it and v3 do not inherit |
| Auth flow | Auth spec | Never | A different source is declared |
| Deploy checklist | None declared (the write returns a `no_declared_inputs` warning) | Never | No recorded inputs; counted in `untracked_content_changes`, not evidence it was unaffected |

The September doc is then retracted (`source_withdrawn`); `wisp impact` still resolves it by path, and `wisp retract` lists the direct dependents. After erasure it resolves by its concept id with no labels.

# SPY options multi-agent playbook

Narrative integration test: `wisp-invariants/tests/spy_options_playbook.rs`.

**Domain**: discretionary **0–2 DTE SPY calls** sized off **news signal rubric** (CPI, Fed, VIX).

**Honest mistakes modeled**

| Mistake | How the test exercises it |
|---------|---------------------------|
| Two agents update **different** sections | Both commit without conflict |
| Two agents update **Exit rules** | Second gets JSON `section_conflict`; resubmit on `theirs.version` |
| Human **reorders** sections in the file | `record` keeps `SectionId`s |
| **Stale** pre-CPI note vs CPI print | Headless update cites CPI observation as `input` |
| **Retract** superseded whisper | `retract` on observation path |
| **Remove** a mistaken log line | `update_section` on journal after human edit |
| Local edit + agent headless write | Disjoint sections: working tree dirty text preserved (I19) |

**Failure modes** (`wisp-invariants/tests/spy_options_failures.rs`):

| Failure | Expected behavior |
|---------|-------------------|
| Hand-edit `index.md` or `strategy/log.md` then `record` | Exit **4** Refused (generated view) |
| `scratch/no-yaml.md` without frontmatter | Exit **5** Validation |
| Second headless update on stale `base_version` | Exit **3** Conflict (repeatable until `theirs`) |
| `assert` on wrong observation `version` | `assertion_failed` conflict |
| `assert_head` after another agent committed | `assertion_failed` on head |
| Duplicate `wisp_id` file, **incremental** `record` | §7.4.1 **move** to copy path (surprise) |
| Duplicate `wisp_id`, **`full` record** | §7.4.2 copy remint + new concept |
| `retract` same observation twice | Conflict or NotFound |
| Headless `update_section` after retract | Conflict or NotFound |
| Headless update on unknown concept path | Exit **2** NotFound |

Fixture bundle: `data/bundles/spy-options/`.

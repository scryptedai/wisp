//! Long help and root `after_help` (contracts point to `wisp doc`).

pub const ROOT_LONG_ABOUT: &str = "\
Local-first differential OKF store: section-level changesets, encrypted payloads, agent-friendly JSON.

Contracts (exit codes, targets, what success does not prove): run `wisp doc cli` or `wisp doc list`.\n\
With --json, stdout is one envelope {ok, command, data|error}; see `wisp doc cli`.";

pub const ROOT_AFTER_HELP: &str = "\
Topic documentation:\n  \
wisp doc list          installed topics and paths\n  \
wisp doc <topic>       print a topic (e.g. erase, merge, search, impact, apply)\n  \
wisp doc path          print the documentation directory\n  \
wisp doc --man         man wisp (uses install-man path via man -M); install: contrib/scripts/install-man.sh\n\n\
Write paths:\n  \
record                 working-tree edits → one changeset\n  \
update, apply, …       headless; see `wisp doc apply`\n\n\
Exit codes: 3 conflict, 4 refused, 5 validation, 7 retryable lock, 8 integrity, 9 key, 10 erase pending.";

pub const RECORD_LONG: &str = "\
Convert **working-tree** file changes into one changeset, then re-render OKF paths.\n\n\
Not the same as headless `update` / `apply` (no tree round-trip). For agent batches see `wisp doc apply`.\n\
Use --resolve after committing merge resolutions from `.wisp/merge.json`.";

pub const APPLY_LONG: &str = "\
Commit a **batch** of headless operations from JSON/YAML (file or stdin) as one changeset.\n\n\
Ops include update_section (full replace, base_version), append_section and prepend_section \
(log fragments, no version precondition), create, set, and others. Truth is hot tier and \
`wisp cat`; disk may lag until `wisp materialize --wait`. With --resolve, section_conflict \
(exit 3) refreshes base_version from hot and retries (bounded). Merge working-tree cleanup \
uses `wisp record --resolve` instead. `add_section` / `update_section` may return \
`same_heading_siblings` on each change (other section ids with the same normalized heading). \
See `wisp doc apply` and `wisp doc AGENT-WRITERS.md`.";

pub const ERASE_LONG: &str = "\
Destroy scope keys and cold payloads for a section, concept, asset, group, or actor.\n\n\
Without --yes: **preview only** (dependents listed). With --yes: irreversible.\n\
--discard overwrites unrecorded edits in managed checkouts. Opening the repo may finish crash-interrupted erase.\n\
Full semantics: `wisp doc erase` and `wisp doc trust`.";

pub const MERGE_LONG: &str = "\
Merge another ref into the current ref (section-level replay and three-way merge).\n\n\
Outcomes include fast-forward, disjoint replay, erase-wins, and **conflict** (exit 3, `.wisp/merge.json`).\n\
--resolutions file: {\"take\":\"ours\"|\"theirs\"} per scope; `take` drops the whole losing chain.\n\
Details: `wisp doc merge`.";

pub const SEARCH_LONG: &str = "\
Retrieve sections on the ref. Modes: **auto**, **exact**, **fuzzy** (BM25+trigram RRF), **keyword** (BM25 only), **ri**, **semantic**. **auto** stops at the first tier with hits: exact → fuzzy → ri → semantic (only if built with semantic and not --local-only).\n\n\
`--field heading|body|title|path` searches one column (title = frontmatter `title`, path = concept path). Hits include `field`, `snippet`, and hot `version` for the next write pin.\n\n\
Manifest embedding backends are not used by built-in search. See `wisp doc search` and `wisp doc extensions`.";

pub const DIFF_LONG: &str = "\
Without --from/--to: working-tree section diff vs the ref head.\n\n\
With --from and --to together:\n\
• **Changeset** ids — scope-level ops between two snapshots (`wisp diff --from <cs> --to <cs>`).\n\
• **Section** ids — when the path is `<concept>#<section>`, ids are section **change** ids; JSON `diff` lists lines in --from absent from --to (draft vs winner on exit 3). See `wisp doc apply`.";

pub const PACK_LONG: &str = "\
Build JSON for LLM context within --budget tokens. Default codec is tok:approx:chars4 (ceil(chars/4)); pack spends ~90% of budget (see JSON codec_note).\n\n\
`wisp doc extensions` for codecs.";

pub const IMPACT_LONG: &str = "\
Forward trace: recorded work that **declared** this source as input, scoped to --ref and --depth.\n\n\
Historical mode only (data.mode is always historical). Empty results mean no recorded dependents in scope—not safety for agents.\n\
Read-only query, but opening the repo may complete pending erase. `wisp doc impact`.";

pub const SERVE_LONG: &str = "\
Replica sync transport: **ColdPack** over --stdio or dumb HTTP pack endpoint.\n\n\
This is not an MCP server. Pair with `wisp push`/`pull` stdio:// URLs. `wisp doc extensions`.";

pub const EXPORT_LONG: &str = "\
Exactly one of: --git <dir>, --archive <file>, --html <dir>, or --verify <dir> on an existing export.\n\n\
--html: static site; --no-content strips bodies; plaintext-copy warning on every page. `wisp doc export-html`.";

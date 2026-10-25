# OKF profiles (`wisp-profile`)

Wisp bundles use profile **`okf-0.2`** by default. Checkout renders a Google Open Knowledge Format v0.2 tree; validation and renderers live in `crates/wisp-profile`.

## Wisp extensions (consumers must tolerate unknown keys)

| Key / marker | Role |
|--------------|------|
| `wisp_id` | Stable concept identity across renames (managed on checkout). |
| `wisp_retracted` | Erasure record when rendering with `--include-retracted`. |
| `<!-- ^section-id -->` | Section anchor in markdown bodies; not part of the okf aspect. |
| `sources[].section` | Section id for a projected input pin; optional on hand-written `sources` too. |

## Validation findings (okf-0.2)

| Code | Level | Meaning |
|------|-------|---------|
| `type_required` | Must | §11.2 |
| `reserved_concept` | Must | Concept path maps to `index.md` or `log.md` |
| `timestamp_format` | Should | RFC 3339 with offset on v0.2 timestamp keys |
| `stale` | Should | `stale_after` is in the past |
| `frontmatter_shadow` | Should | Preamble repeats a frontmatter scalar with a different value |

See `wisp validate` and `docs/CLI.md`.

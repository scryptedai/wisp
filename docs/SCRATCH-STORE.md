# Local scratch OKF store (`docs/test/`)

`docs/test/` is **gitignored**. It is an empty wisp bundle root: `wisp init` only (no sample concepts). Add your own OKF markdown, then record.

```sh
export WISP_ACTOR=human:dev
./target/release/wisp --repo docs/test status
# after adding *.md concepts:
./target/release/wisp --repo docs/test record -m "message"
```

Reset to empty: delete everything under `docs/test/` and run `wisp init` there again.

**Desk demo** (clone → merge → erase → replica): [`demo/desk/README.md`](../demo/desk/README.md) — clones into `demo/workspaces/` from this bundle.

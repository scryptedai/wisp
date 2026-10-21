# Local scratch OKF store (`docs/test/`)

`docs/test/` is **gitignored**. Initialize the bundle **inside** that directory (not the repo root):

```sh
mkdir -p docs/test
cd docs/test
wisp init
```

Add your own OKF markdown under `docs/test/`, then record from the repo root or with `--repo`:

```sh
export WISP_ACTOR=human:dev
./target/release/wisp --repo docs/test status
./target/release/wisp --repo docs/test record -m "message"
```

Reset to empty:

```sh
rm -rf docs/test
mkdir -p docs/test && cd docs/test && wisp init
```

**Desk demo** (clone → merge → erase → replica): [`demo/desk/README.md`](../demo/desk/README.md) — clones into `demo/workspaces/` from this bundle.

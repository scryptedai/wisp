# Contributing to wisp (Community Release)

Thank you for helping improve wisp. This project is **Scrypted’s GPLv2 Community Release** — contributions are accepted under the same license (see [LICENSE](LICENSE)).

## Before you start

- Read [README.md](README.md) and [AGENTS.md](AGENTS.md) (also used by review bots).
- Behavior changes: read [DECISIONS.md](DECISIONS.md) and the v1 design PDF (maintainers).
- Bugs and features: open an issue or discuss on the PR if the change is large.

## Development setup

```sh
git clone https://github.com/scryptedai/wisp.git
cd wisp
rustc --version   # 1.80+
cargo build
cargo test --workspace
```

## Git worktrees (recommended, Git-style)

Use **git worktrees** when you juggle multiple branches without stashing — same idea as [Git’s own workflow](https://git-scm.com/docs/git-worktree):

```sh
# From your primary clone:
git fetch origin
git worktree add ../wisp-feature-a -b feature/my-change origin/main
cd ../wisp-feature-a
cargo test --workspace

# Second topic in parallel:
git worktree add ../wisp-feature-b -b fix/other origin/main
```

Rules of thumb:

- One worktree directory per branch; build artifacts live in each tree’s `target/` (or set `CARGO_TARGET_DIR` to a shared path if you prefer).
- Remove when done: `git worktree remove ../wisp-feature-a` (after merge or abandon).
- **`wisp worktree`** in the product is unrelated — it manages OKF bundle checkouts, not this source repo.

## Patch submission (pull requests)

Inspired by [Git’s patch submission](https://git-scm.com/docs/SubmittingPatches):

1. **Fork** and branch from `main`, or use a worktree as above.
2. **One logical change per PR** when possible (easier review, cleaner history).
3. **Write a clear subject and body** — what problem, what approach, how tested.
4. **Run CI locally** before push:

   ```sh
   cargo fmt --all -- --check
   cargo clippy --all-targets -- -D warnings
   cargo test --workspace
   cargo test -p wisp-invariants
   ```

5. **Update docs** when you change user-visible behavior (`README`, `DECISIONS.md`, `docs/`).
6. **Sign-off**: by opening a PR you license your contribution under **GPLv2** (same as the project).

We merge via **GitHub pull requests**; rebase or merge commits are both acceptable if history stays readable.

## Tests and corpus

- **Unit / engine**: `cargo test -p wisp-engine` (and other crates).
- **Integration gate**: `cargo test -p wisp-invariants` — must pass for behavior that touches OKF workflows.
- New scenarios: add under `crates/wisp-corpus/` and register in `catalog.toml`.

## Code review

- GitHub Actions runs [.github/workflows/ci.yml](.github/workflows/ci.yml) on every PR.
- [CodeRabbit](https://coderabbit.ai) (or similar) may comment if enabled on the org repo — config in [.coderabbit.yaml](.coderabbit.yaml).
- Maintainers may request changes; address or discuss before merge.

## Community vs other editions

This repository is only the **Community Release**. Scrypted may ship other editions under separate terms; do not assume those terms apply here.

## Questions

Open a GitHub issue or contact the maintainers listed in [NOTICE](NOTICE).

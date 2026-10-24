# Contributing to wisp (Community Release)

Thank you for helping improve wisp. This project is **Scrypted’s GPLv2 Community Release** (see [LICENSE](LICENSE)).

## Contribution terms

**By opening a pull request, posting a patch, or otherwise submitting work to this repository, you agree to the following.**

1. **Community Release (GPLv2).** Your contribution may be merged and distributed with this tree under the **GNU General Public License version 2** (or, if you clearly mark otherwise, a GPL-compatible license that allows inclusion in GPLv2 works).

2. **Grant to Scrypted.** You grant **Scrypted, Inc.** and its affiliates a **perpetual, worldwide, non-exclusive, royalty-free, irrevocable** license to use, reproduce, modify, merge, distribute, and sublicense your contribution—including in **other editions or products** of wisp or related offerings that are **not** limited to GPLv2—without further permission or compensation from you.

3. **Authority.** You created the contribution or have the right to submit it, and it does not knowingly infringe third-party rights or violate agreements that would block the grants above.

4. **Employer / agent.** If you contribute on behalf of an employer or client, you represent that you are authorized to make these grants on their behalf (or that their policies allow it).

If you cannot agree, **do not submit** contributions here; contact the maintainers in [NOTICE](NOTICE) to discuss alternatives.

These terms are in addition to GPLv2 for what ships in the Community Release; they clarify Scrypted’s inbound rights so community improvements can also be used in separately licensed offerings, as described in [README.md](README.md).

## Before you start

- Read [README.md](README.md) and [AGENTS.md](AGENTS.md) (also used by review bots).
- Behavior changes: read [DECISIONS.md](DECISIONS.md), [docs/MERGE.md](docs/MERGE.md), and [docs/SEARCH.md](docs/SEARCH.md) / [docs/EXTENSIONS.md](docs/EXTENSIONS.md) when retrieval or hooks change.
- Bugs and features: open an issue or discuss on the PR if the change is large.

## Development setup

```sh
git clone https://github.com/scryptedai/wisp.git
cd wisp
rustc --version   # 1.88+ (`rust-toolchain.toml` pins the workspace minimum)
cargo build
cargo test --workspace
```

## Branching (GitHub Flow)

We use **GitHub Flow**, similar to the [Git project](https://git-scm.com/docs/gitworkflows): one long-lived integration branch, short-lived topic branches, merge via PR.

| Branch | Role |
|--------|------|
| **`main`** | Default branch; always buildable; CI on every push and PR; release tags point here. |
| **`feature/…`, `fix/…`, `docs/…`, `chore/…`** | Your change; branch from `main`, open a PR back to `main`, delete after merge. |

There is **no** `develop` or `release/*` integration line for now. Maintainers and contributors use the same model.

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
4. **Run CI locally** before push (mirrors [.github/workflows/ci.yml](.github/workflows/ci.yml)):

   ```sh
   ./contrib/ci-check.sh
   ```

   Optional git hooks (fmt on commit, full CI on push):

   ```sh
   ./contrib/install-git-hooks.sh
   ```

5. **Update docs** when you change user-visible behavior (`README`, `DECISIONS.md`, `docs/`).
6. **Contribution terms**: confirm you agree to [Contribution terms](#contribution-terms) (GPLv2 for this tree + grant to Scrypted for other editions).

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

This repository is only the **Community Release** (GPLv2). Scrypted may ship **other editions** under separate terms; those editions are not covered by this repo’s license. The [contribution grant](#contribution-terms) is what allows Scrypted to reuse merged community work outside GPLv2 when permitted by that grant.

## Questions

Open a GitHub issue or contact the maintainers listed in [NOTICE](NOTICE).

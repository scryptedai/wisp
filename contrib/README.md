# contrib

Files that belong in dot-directories, which the delivery tooling could not write there directly:

- `claude-skill/project-memory/SKILL.md` → copy to `.claude/skills/project-memory/SKILL.md` in any repository with a `.wisp/` to teach Claude Code to use wisp as shared memory.
- `github-workflows/ci.yml` → copy to `.github/workflows/ci.yml` for fmt, clippy, tests, and a release build on Linux, macOS, and Windows.
- `scripts/strip-commit-attribution.sh` → optional maintainer tool: `git-filter-repo` pass to remove `Co-authored-by: Cursor` / “Made with Cursor” lines from **all** commit messages (rewrites history; force-push required).
- `scripts/install-local.sh` → build release `wisp`, install to `~/.cargo/bin`, and copy docs to `~/.local/share/wisp/doc` (re-run to update). Use `--verify` for a CI-like gate; `--no-semantic` for air-gapped builds; `--prefix /usr/local` for FHS layout (`/usr/local/share/doc/wisp`).
- `scripts/install-docs.sh` → documentation only (same destinations as above).
- `contrib/man/wisp.1` → manual page; `scripts/install-man.sh` → `~/.local/share/man/man1` (set `MANPATH` or use `wisp doc --man` / `man -l`).
- `scripts/wisp-dev.sh` → `cargo run -p wisp-cli` from the repo root (no install).

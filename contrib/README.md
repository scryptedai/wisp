# contrib

Files that belong in dot-directories, which the delivery tooling could not write there directly:

- `claude-skill/project-memory/SKILL.md` → copy to `.claude/skills/project-memory/SKILL.md` in any repository with a `.wisp/` to teach Claude Code to use wisp as shared memory.
- `github-workflows/ci.yml` → copy to `.github/workflows/ci.yml` for fmt, clippy, tests, and a release build on Linux, macOS, and Windows.
- `scripts/strip-commit-attribution.sh` → optional maintainer tool: `git-filter-repo` pass to remove `Co-authored-by: Cursor` / “Made with Cursor” lines from **all** commit messages (rewrites history; force-push required).

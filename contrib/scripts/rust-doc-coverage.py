#!/usr/bin/env python3
"""Doc coverage on changed pub items in crate library sources (CodeRabbit-style gate).

Counts `pub fn` / `pub struct` / `pub enum` / `pub type` / `pub const` in paths under
crates/*/src, excluding integration test trees. Each item needs a `///` doc line
immediately above (attributes between doc and item are allowed).

Exemptions align with .coderabbit.yaml path_instructions (e.g. wisp-invariants tests).
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

PUB_ITEM = re.compile(
    r"^\s*pub(?!\(crate\))(?:\s*\([^)]*\))?\s+(?:async\s+)?(?:unsafe\s+)?fn\s+\w+"
)
PUB_TYPE = re.compile(
    r"^\s*pub(?!\(crate\))(?:\s*\([^)]*\))?\s+(?:struct|enum|type|const)\s+\w+"
)

# Clap long-help literals; documenting each const duplicates the string body.
EXEMPT_BASENAMES = frozenset({"help_text.rs", "cli.rs"})


def is_doc_line(line: str) -> bool:
    s = line.strip()
    return s.startswith("///") or s.startswith("//!")


def has_doc_above(lines: list[str], idx: int) -> bool:
    j = idx - 1
    while j >= 0 and lines[j].strip() == "":
        j -= 1
    while j >= 0:
        s = lines[j].strip()
        if is_doc_line(lines[j]):
            return True
        if s.startswith("#[") or s == "":
            j -= 1
            continue
        break
    return False


def should_check_file(path: Path) -> bool:
    if path.name in EXEMPT_BASENAMES:
        return False
    parts = path.parts
    if "wisp-invariants" in parts:
        return False
    if path.name.endswith("_tests.rs"):
        return False
    if "/tests/" in path.as_posix():
        return False
    if "/src/" not in path.as_posix() or path.suffix != ".rs":
        return False
    return path.is_file()


def check_file(
    path: Path, only_lines: set[int] | None
) -> tuple[int, list[tuple[int, str]]]:
    text = path.read_text(encoding="utf-8")
    lines = text.splitlines()
    undocumented: list[tuple[int, str]] = []
    total = 0
    for i, line in enumerate(lines):
        line_no = i + 1
        if only_lines is not None and line_no not in only_lines:
            continue
        if "pub use" in line:
            continue
        if not (PUB_ITEM.match(line) or PUB_TYPE.match(line)):
            continue
        total += 1
        if not has_doc_above(lines, i):
            undocumented.append((line_no, line.strip()))
    return total, undocumented


def git_diff_added_lines(base: str | None, staged: bool) -> dict[Path, set[int]]:
    """Map each path to 1-based line numbers introduced in the diff (+ lines)."""
    if staged:
        cmd = ["git", "diff", "--cached", "-U0", "--diff-filter=ACMR"]
    elif base:
        cmd = ["git", "diff", f"{base}...HEAD", "-U0", "--diff-filter=ACMR"]
    else:
        cmd = ["git", "diff", "HEAD", "-U0", "--diff-filter=ACMR"]
    out = subprocess.run(cmd, capture_output=True, text=True, check=True)
    added: dict[Path, set[int]] = {}
    current: Path | None = None
    new_line = 0
    for raw in out.stdout.splitlines():
        if raw.startswith("+++ b/"):
            current = Path(raw[6:])
            added.setdefault(current, set())
            new_line = 0
            continue
        if current is None or raw.startswith("---") or raw.startswith("diff "):
            continue
        if raw.startswith("@@"):
            m = re.search(r"\+(\d+)", raw)
            new_line = int(m.group(1)) - 1 if m else 0
            continue
        if raw.startswith("+") and not raw.startswith("+++"):
            new_line += 1
            added.setdefault(current, set()).add(new_line)
        elif raw.startswith(" "):
            new_line += 1
        # Deletions ('-') do not advance the new-file line counter.
    return {p: lines for p, lines in added.items() if lines}


def resolve_base(explicit: str | None) -> str | None:
    if explicit:
        return explicit
    for candidate in ("origin/main", "main"):
        r = subprocess.run(
            ["git", "rev-parse", "--verify", candidate],
            capture_output=True,
            text=True,
        )
        if r.returncode == 0:
            return candidate
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--base",
        help="Merge base for branch diff (default: origin/main or main)",
    )
    parser.add_argument(
        "--staged",
        action="store_true",
        help="Only staged paths (pre-commit)",
    )
    parser.add_argument(
        "--threshold",
        type=float,
        default=0.80,
        help="Minimum documented fraction (default 0.80)",
    )
    args = parser.parse_args()

    base = None if args.staged else resolve_base(args.base)
    if not args.staged and base is None:
        print("rust-doc-coverage: no main branch ref; skip", file=sys.stderr)
        return 0

    try:
        added_by_file = git_diff_added_lines(base, args.staged)
    except subprocess.CalledProcessError:
        if base and not args.staged:
            subprocess.run(
                ["git", "fetch", "origin", base.removeprefix("origin/")],
                capture_output=True,
                text=True,
            )
            try:
                added_by_file = git_diff_added_lines(base, args.staged)
            except subprocess.CalledProcessError as e:
                print(f"rust-doc-coverage: git failed: {e}", file=sys.stderr)
                return 1
        else:
            print("rust-doc-coverage: git diff failed", file=sys.stderr)
            return 1

    rust_files = [f for f in added_by_file if should_check_file(f)]
    if not rust_files:
        print("rust-doc-coverage: no touched library sources (skip)")
        return 0

    grand_total = 0
    grand_doc = 0
    all_undoc: list[tuple[Path, int, str]] = []
    for f in rust_files:
        total, undoc = check_file(f, added_by_file[f])
        if total == 0:
            continue
        grand_total += total
        grand_doc += total - len(undoc)
        for line_no, text in undoc:
            all_undoc.append((f, line_no, text))

    if grand_total == 0:
        print("rust-doc-coverage: ok (no pub items in diff)")
        return 0

    ratio = grand_doc / grand_total
    print(
        f"rust-doc-coverage: {grand_doc}/{grand_total} ({ratio:.0%}) "
        f"on touched lines in {len(rust_files)} file(s)"
    )
    if ratio < args.threshold:
        print(
            f"rust-doc-coverage: FAIL (need >={args.threshold:.0%}; add /// on pub items)",
            file=sys.stderr,
        )
        for path, ln, text in all_undoc[:25]:
            print(f"  {path}:{ln}: {text}", file=sys.stderr)
        if len(all_undoc) > 25:
            print(f"  ... and {len(all_undoc) - 25} more", file=sys.stderr)
        return 1
    print("rust-doc-coverage: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())

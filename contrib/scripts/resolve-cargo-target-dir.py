#!/usr/bin/env python3
"""Resolve CARGO_TARGET_DIR from workspace metadata and optional local wisp-target.toml."""

from __future__ import annotations

import os
import sys
from pathlib import Path

try:
    import tomllib
except ImportError:  # pragma: no cover
    import tomli as tomllib  # type: ignore


def _expand(path: str) -> Path:
    return Path(os.path.expanduser(os.path.expandvars(path))).resolve()


def xdg_cache() -> Path:
    return Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache"))


def read_workspace_meta(root: Path) -> dict:
    cargo = root / "Cargo.toml"
    if not cargo.is_file():
        return {}
    data = tomllib.loads(cargo.read_text(encoding="utf-8"))
    return data.get("workspace", {}).get("metadata", {}).get("wisp-dev", {})


def read_local(root: Path) -> dict:
    for name in ("wisp-target.toml", "contrib/dev-target.local.toml"):
        p = root / name
        if p.is_file():
            return tomllib.loads(p.read_text(encoding="utf-8"))
    return {}


def resolve(root: Path) -> Path:
    if os.environ.get("CI") or os.environ.get("GITHUB_ACTIONS"):
        return (root / "target").resolve()

    explicit = os.environ.get("WISP_CARGO_TARGET_DIR")
    if explicit:
        return _expand(explicit)

    if os.environ.get("WISP_TARGET_PER_WORKTREE") in ("1", "true", "yes"):
        return (root / "target").resolve()

    local = read_local(root)
    meta = read_workspace_meta(root)

    build = local.get("build", {})
    mode = (
        build.get("mode")
        or meta.get("target")
        or "shared"
    ).lower()

    if mode in ("worktree", "per-worktree", "tree"):
        return (root / "target").resolve()

    shared = build.get("shared_dir") or meta.get("shared-dir")
    if shared:
        return _expand(str(shared))

    sub = meta.get("shared-cache-subdir", "wisp/target")
    return (xdg_cache() / sub).resolve()


def main() -> int:
    root = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else Path.cwd()
    print(resolve(root))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Run the search IR battery against a wisp repo (must match search_battery.rs assertions).

Canonical cases: contrib/search-battery/cases.toml
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent
CASES_PATH = ROOT / "cases.toml"

MODES_DEFAULT = ("auto", "fuzzy", "keyword", "ri", "semantic")


@dataclass(frozen=True)
class Case:
    id: str
    query: str
    anchor: str
    category: str
    expect: str
    keyword_in_top_k: bool
    auto_semantic_top1: bool
    note: str


def load_cases() -> tuple[int, list[Case]]:
    with CASES_PATH.open("rb") as f:
        doc = tomllib.load(f)
    k = int(doc["k"])
    cases = [
        Case(
            id=c["id"],
            query=c["query"],
            anchor=c["anchor"],
            category=c["category"],
            expect=c["expect"],
            keyword_in_top_k=bool(c["keyword_in_top_k"]),
            auto_semantic_top1=bool(c["auto_semantic_top1"]),
            note=c.get("note", ""),
        )
        for c in doc["cases"]
    ]
    return k, cases


def wisp_search(
    query: str,
    mode: str,
    k: int,
    local_only: bool,
) -> dict:
    cmd = ["wisp", "search", query, "--mode", mode, "-k", str(k), "--json"]
    if local_only:
        cmd.append("--local-only")
    res = subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        cwd=os.environ.get("WISP_BUNDLE_ROOT", "."),
    )
    if res.returncode != 0:
        raise RuntimeError(f"wisp search failed: {res.stderr or res.stdout}")
    data = json.loads(res.stdout)
    return data.get("data", data)


def anchor_rank(hits: list[dict], anchor: str) -> int | None:
    for i, h in enumerate(hits):
        heading = h.get("heading") or ""
        path = h.get("path") or ""
        if anchor in heading or anchor in path:
            return i + 1
    return None


def should_assert(mode: str, c: Case) -> bool:
    if c.expect == "top1":
        return mode != "ri"
    if c.expect == "topK":
        if mode == "keyword":
            return c.keyword_in_top_k
        if mode == "ri":
            return False
        return mode in ("auto", "fuzzy")
    if c.expect == "miss@K":
        return True
    return False


def neural_top1_override(c: Case, mode: str, neural: bool, local_only: bool) -> bool:
    if local_only and mode == "auto":
        return False
    return (
        c.expect == "miss@K"
        and c.auto_semantic_top1
        and neural
        and mode in ("auto", "semantic")
    )


def expect_label(c: Case, mode: str, neural: bool, local_only: bool) -> str:
    if neural_top1_override(c, mode, neural, local_only):
        return "top1@neural"
    return c.expect


def assert_case(
    c: Case,
    mode: str,
    k: int,
    rank: int | None,
    results: list[dict],
    mode_used: str | None,
    neural: bool,
    local_only: bool,
) -> None:
    if not should_assert(mode, c):
        return
    if c.expect == "top1":
        if rank != 1:
            raise AssertionError(f"{c.id} [{mode}] expected rank 1 for {c.anchor}, got {rank}")
        return
    if c.expect == "topK":
        if rank is None:
            raise AssertionError(f"{c.id} [{mode}] expected {c.anchor} in top {k}")
        return
    if c.expect == "miss@K":
        if neural_top1_override(c, mode, neural, local_only):
            if rank != 1:
                raise AssertionError(
                    f"{c.id} [{mode}] expected rank 1 for {c.anchor} (neural), got {rank}"
                )
            if mode == "auto" and mode_used != "semantic":
                raise AssertionError(
                    f"{c.id} [auto] expected mode_used semantic, got {mode_used}"
                )
            return
        if rank is not None:
            raise AssertionError(
                f"{c.id} [{mode}] expected miss for {c.anchor} in top {k}, rank {rank}"
            )
        if mode in ("auto", "fuzzy") and results:
            raise AssertionError(
                f"{c.id} [{mode}] expected empty results (no ghost hits), got {len(results)}"
            )


def run_mode(
    mode: str,
    k: int,
    cases: list[Case],
    neural: bool,
    local_only: bool,
) -> int:
    """Run one mode; return number of failed assertions."""
    failures = 0
    by_cat: dict[str, dict[str, float]] = {}
    print(f"\n=== mode: {mode} (local_only={local_only}, neural_contract={neural}) ===")
    print(f"{'id':8} {'category':22} {'expect':12} {'rank':6} {'top':}")
    for c in cases:
        try:
            auto_local = local_only and mode == "auto"
            payload = wisp_search(c.query, mode, k, local_only=auto_local)
        except RuntimeError as e:
            print(f"{c.id:8} ERROR {e}")
            failures += 1
            continue
        results = payload.get("results") or []
        mode_used = payload.get("mode")
        rank = anchor_rank(results, c.anchor)
        top = results[0] if results else {}
        label = expect_label(c, mode, neural, auto_local)
        print(
            f"{c.id:8} {c.category:22} {label:12} {str(rank or '—'):>6}  "
            f"{top.get('path', '-')}"
        )
        if c.expect != "report":
            st = by_cat.setdefault(
                c.category, {"total": 0, "hit_at_1": 0, "hit_at_k": 0, "mrr": 0.0}
            )
            st["total"] += 1
            if rank is not None:
                st["hit_at_k"] += 1
                if rank == 1:
                    st["hit_at_1"] += 1
                st["mrr"] += 1.0 / rank
        try:
            assert_case(c, mode, k, rank, results, mode_used, neural, auto_local)
        except AssertionError as e:
            print(f"         FAIL: {e}")
            failures += 1
    print(f"\n--- summary ({mode}) ---")
    for cat, st in sorted(by_cat.items()):
        n = st["total"]
        if n == 0:
            continue
        print(f"[{cat.upper()}] N={int(n)}")
        print(f"  Recall@1: {st['hit_at_1'] / n:.2%}")
        print(f"  Recall@{k}: {st['hit_at_k'] / n:.2%}")
        print(f"  MRR:      {st['mrr'] / n:.3f}")
    return failures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "mode",
        nargs="?",
        default="auto",
        help=f"search mode or 'all' ({', '.join(MODES_DEFAULT)})",
    )
    parser.add_argument(
        "--airgap",
        action="store_true",
        help="Expect no neural tier (matches cargo test --no-default-features)",
    )
    parser.add_argument(
        "--local-only",
        action="store_true",
        help="Pass --local-only to wisp for auto mode (T3.x must miss)",
    )
    args = parser.parse_args()
    k, cases = load_cases()
    neural = not args.airgap
    modes = MODES_DEFAULT if args.mode == "all" else (args.mode,)
    total_fail = 0
    for mode in modes:
        total_fail += run_mode(mode, k, cases, neural, args.local_only)
    if args.local_only:
        total_fail += run_local_only_gap_checks(k, cases)
    if total_fail:
        print(f"\n{battery_failures(total_fail)}", file=sys.stderr)
        return 1
    print("\nOK: all assertions passed")
    return 0


def battery_failures(n: int) -> str:
    return f"BATTERY FAILED: {n} assertion(s)"


def run_local_only_gap_checks(k: int, cases: list[Case]) -> int:
    """Mirror Rust: semantic_gap probes miss on auto with local_only."""
    failures = 0
    print("\n=== auto + --local-only (semantic_gap probes) ===")
    for c in cases:
        if c.expect != "miss@K":
            continue
        payload = wisp_search(c.query, "auto", k, local_only=True)
        results = payload.get("results") or []
        rank = anchor_rank(results, c.anchor)
        if rank is not None:
            print(f"{c.id} FAIL: expected miss, rank {rank}")
            failures += 1
        else:
            print(f"{c.id} ok (miss)")
    return failures


if __name__ == "__main__":
    raise SystemExit(main())

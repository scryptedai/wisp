#!/usr/bin/env python3
"""Run the search IR battery against a wisp repo (cwd = bundle root)."""

import json
import os
import subprocess
import sys
from dataclasses import dataclass
from typing import Literal

K = 5

Expect = Literal["top1", "topK", "miss@K", "report"]


@dataclass(frozen=True)
class Case:
    id: str
    query: str
    anchor: str
    category: str
    expect: Expect


CASES = [
    Case("T1.0", "database failover standby replica", "sec-failover", "lexical", "top1"),
    Case("T2.1", "databse replca hearbeat", "sec-failover", "syntactic_fuzzy", "topK"),
    Case("T2.2a", "out of memory", "sec-oom", "syntactic", "topK"),
    Case("T2.2b", "out-of-memory", "sec-oom", "syntactic", "topK"),
    Case("T3.1", "RAM depleted process terminated", "sec-oom", "semantic_gap", "miss@K"),
    Case("T3.2", "GDPR right to be forgotten compliance", "sec-crypto-shred", "semantic_gap", "miss@K"),
    Case("T4.1", "capital starvation bank liquidity", "sec-liquidity", "polysemy", "report"),
    Case("T4.2", "operating system thread context switch", "sec-trading-pipeline", "polysemy", "report"),
    Case("T5.1", "OOM kernel kill", "sec-oom", "acronym", "report"),
]


def wisp_search(query: str, mode: str = "hybrid", k: int = K) -> list[dict]:
    cmd = ["wisp", "search", query, "--mode", mode, "--limit", str(k), "--json"]
    res = subprocess.run(cmd, capture_output=True, text=True, cwd=os.environ.get("WISP_BUNDLE_ROOT", "."))
    if res.returncode != 0:
        return []
    data = json.loads(res.stdout)
    return data.get("data", data).get("results", [])


def anchor_rank(hits: list[dict], anchor: str) -> int | None:
    for i, h in enumerate(hits):
        heading = h.get("heading") or ""
        path = h.get("path") or ""
        if anchor in heading or anchor in path:
            return i + 1
    return None


def main() -> int:
    mode = sys.argv[1] if len(sys.argv) > 1 else "hybrid"
    by_cat: dict[str, dict[str, float]] = {}
    print(f"mode={mode} k={K}")
    for c in CASES:
        hits = wisp_search(c.query, mode=mode)
        rank = anchor_rank(hits, c.anchor)
        top = hits[0] if hits else {}
        print(
            f"{c.id:8} {c.category:22} expect={c.expect:8} rank={rank or '—':>4}  "
            f"top={top.get('path', '-')}"
        )
        if c.expect == "report":
            continue
        st = by_cat.setdefault(c.category, {"total": 0, "hit_at_1": 0, "hit_at_k": 0, "mrr": 0.0})
        st["total"] += 1
        if rank is not None:
            st["hit_at_k"] += 1
            if rank == 1:
                st["hit_at_1"] += 1
            st["mrr"] += 1.0 / rank
    print(f"\n--- summary ({mode}) ---")
    for cat, st in sorted(by_cat.items()):
        n = st["total"]
        if n == 0:
            continue
        print(f"[{cat.upper()}] N={n}")
        print(f"  Recall@1: {st['hit_at_1'] / n:.2%}")
        print(f"  Recall@{K}: {st['hit_at_k'] / n:.2%}")
        print(f"  MRR:      {st['mrr'] / n:.3f}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

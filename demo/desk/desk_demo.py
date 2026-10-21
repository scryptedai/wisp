#!/usr/bin/env python3
"""
Agent desk demo: accountable autonomy on a SPY options wisp bundle.

Uses only `wisp` CLI commits (plus optional LLM draft). See README.md.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import textwrap
import time
from pathlib import Path

from scrypted_llm import complete_text, load_demo_env

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_SOURCE = REPO_ROOT / "docs" / "test"
DEFAULT_DEMO = REPO_ROOT / "demo" / "workspaces" / "primary"
DEFAULT_REPLICA = REPO_ROOT / "demo" / "workspaces" / "replica"
FIXTURE_FEED = Path(__file__).resolve().parent / "fixtures" / "oct14_feed.json"
BEAR_REF = "task/bear-demo"


class Wisp:
    def __init__(self, repo: Path, actor: str, bin_path: Path) -> None:
        self.repo = repo
        self.actor = actor
        self.bin = bin_path

    def run(self, *args: str, check: bool = True) -> dict:
        env = os.environ.copy()
        env["WISP_ACTOR"] = self.actor
        cmd = [str(self.bin), "--repo", str(self.repo), "--json", *args]
        p = subprocess.run(cmd, capture_output=True, text=True, env=env)
        out = (p.stdout or "").strip()
        err = (p.stderr or "").strip()
        data: dict = {}
        if out:
            try:
                data = json.loads(out.splitlines()[-1])
            except json.JSONDecodeError:
                data = {"raw": out}
        if check and p.returncode != 0:
            if data.get("ok") is False and data.get("error", {}).get("code") == "conflict":
                return data
            raise RuntimeError(
                f"wisp failed ({p.returncode}): {args}\n{out}\n{err}"
            )
        if check and data.get("ok") is False:
            raise RuntimeError(
                f"wisp failed: {args}\n{out}\n{err}"
            )
        data["_exit"] = p.returncode
        data["_stderr"] = err
        return data

    def run_allow_fail(self, *args: str) -> dict:
        return self.run(*args, check=False)


def wisp_bin() -> Path:
    p = os.environ.get("WISP_BIN")
    if p:
        return Path(p)
    cand = REPO_ROOT / "target" / "release" / "wisp"
    if cand.is_file():
        return cand
    cand = REPO_ROOT / "target" / "debug" / "wisp"
    if cand.is_file():
        return cand
    raise SystemExit("Build wisp first: cargo build -p wisp-cli --release (or set WISP_BIN)")


def step(title: str) -> None:
    print(f"\n{'=' * 72}\n{title}\n{'=' * 72}", flush=True)


def seed(source: Path, demo: Path, replica: Path, fresh: bool) -> None:
    step("SEED: clone SPY learning bundle")
    if not source.is_dir():
        raise SystemExit(f"Missing source bundle {source} (run learning ingest on docs/test first)")
    if fresh:
        for d in (demo, replica):
            if d.exists():
                shutil.rmtree(d)
    if not demo.exists():
        shutil.copytree(source, demo)
        print(f"Copied {source} → {demo}")
    else:
        print(f"Using existing {demo} (--fresh to reset)")
    if not replica.exists():
        shutil.copytree(demo, replica)
        print(f"Cloned replica {replica}")


def cat_sections(w: Wisp, path: str) -> list[dict]:
    d = w.run("cat", path, "--json")
    return d["data"]["sections"]


def act1_live_stub(w: Wisp, use_llm: bool) -> None:
    step("ACT 1 — Live stub ingest with provenance (hawk agent)")
    feed = json.loads(FIXTURE_FEED.read_text())
    obs = "observations/2026-10-14-spy-options-session"
    prior = "observations/2026-10-13-spy-options-session"
    sections = cat_sections(w, obs)
    cash = next(s for s in sections if s.get("heading") == "Cash session")
    after = cash["section"]

    body = draft_live_stub(feed, use_llm)
    w.run(
        "add-section",
        obs,
        f"--after={after}",
        "--heading=Live desk stub (demo)",
        f"--content={body}",
        "--input",
        prior,
        "-m",
        "Hawk: intraday break stub @ 10:35",
    )
    print(w.run("log", "-n", "1"))
    print(w.run("why", obs, "--depth", "1"))


def draft_live_stub(feed: dict, use_llm: bool) -> str:
    if use_llm:
        load_demo_env()
        prompt = f"Write 3 markdown bullet lines for a SPY desk observation stub: {json.dumps(feed)}"
        try:
            text = complete_text(
                prompt,
                "Desk analyst voice. No preamble.",
            )
            if text:
                return text.strip()
        except Exception as e:
            print(f"(Scrypted LLM fallback: {e})", file=sys.stderr)
        if os.environ.get("OPENAI_API_KEY"):
            try:
                import urllib.request

                req = urllib.request.Request(
                    "https://api.openai.com/v1/chat/completions",
                    data=json.dumps(
                        {
                            "model": "gpt-4o-mini",
                            "messages": [{"role": "user", "content": prompt}],
                        }
                    ).encode(),
                    headers={
                        "Authorization": f"Bearer {os.environ['OPENAI_API_KEY']}",
                        "Content-Type": "application/json",
                    },
                )
                with urllib.request.urlopen(req, timeout=30) as resp:
                    out = json.load(resp)
                return out["choices"][0]["message"]["content"]
            except Exception as e:
                print(f"(OpenAI fallback: {e})", file=sys.stderr)

    return textwrap.dedent(
        f"""
        - **Stub last**: {feed['last']} at {feed['as_of']} (demo feed, not live).
        - **Context**: Below session open {feed['open']}; probing low ~{feed['session_low']}.
        - **Desk**: Hawk agent flags **break risk** under 772; cites prior session as input.
        """
    ).strip()


def act2_merge_fight(w: Wisp) -> None:
    step("ACT 2 — Two agents, one section, merge resolution")
    path = "strategy/2026-10-14-ex-post-optimal-play"
    sec = cat_sections(w, path)[0]
    sid, ver = sec["section"], sec["version"]
    target = f"{path}#{sid}"

    refs = w.run("branch")["data"]
    if not any(r["ref"] == BEAR_REF for r in refs):
        w.run("branch", BEAR_REF)

    bear = Wisp(w.repo, "process:bear/1.0", w.bin)
    bear.run("switch", BEAR_REF)
    bear.run(
        "update",
        target,
        f"--base={ver}",
        "-m",
        "Bear: fade breakdown",
        "--content=**Ex-post desk answer**: *Noise — range 766–776 holds; fade breakdown narrative.*",
    )

    hawk = Wisp(w.repo, "process:hawk/1.0", w.bin)
    hawk.run("switch", "main")
    # refresh version on main after bear branch diverged only on bear - main still old ver
    sec_main = cat_sections(hawk, path)[0]
    hawk.run(
        "update",
        target,
        f"--base={sec_main['version']}",
        "-m",
        "Hawk: breakdown confirmed",
        "--content=**Ex-post desk answer**: *Break 772 confirmed; puts and short delta paid.*",
    )

    conflict = hawk.run_allow_fail("merge", BEAR_REF, "--into", "main")
    print(json.dumps(conflict, indent=2)[:1200])
    if conflict.get("ok"):
        print("(merge already resolved — skipping)")
        return

    rows = conflict["error"]["details"]["conflicts"]["conflicts"]
    row = rows[0]
    res_path = w.repo / ".wisp" / "demo-resolutions.json"
    res_path.write_text(
        json.dumps(
            {
                "resolutions": [
                    {
                        "ours": row["ours"],
                        "theirs": row["theirs"],
                        "take": "theirs",
                    }
                ]
            }
        )
    )
    merged = hawk.run("merge", BEAR_REF, "--into", "main", "--resolutions", str(res_path))
    print(json.dumps(merged, indent=2))
    print(hawk.run("search", "fade breakdown", "--mode", "keyword"))


def act3_erase(w: Wisp, demo: Path, replica: Path) -> None:
    step("ACT 3 — Erase max-pain claim; replica sync")
    obs = "observations/2026-10-16-spy-options-session"
    opt = next(s for s in cat_sections(w, obs) if s.get("heading") == "Options")
    target = f"{obs}#{opt['section']}"

    before = w.run("search", "max pain", "--mode", "keyword")
    print("search before:", json.dumps(before["data"]["results"][:2], indent=2))

    w.run(
        "erase",
        target,
        "--reason",
        "policy",
        "-y",
        "--discard",
    )

    after = w.run("search", "max pain", "--mode", "keyword")
    hits = after["data"].get("results", [])
    print("search after:", len(hits), "hits (journal may still mention max pain)")
    obs_hits = [h for h in hits if h["path"] == obs]
    print("erased observation still in search:", bool(obs_hits))

    md = demo / "observations" / "2026-10-16-spy-options-session.md"
    text = md.read_text() if md.is_file() else ""
    print("managed file contains 'max pain':", "max pain" in text.lower())

    # Sync erasure to replica (file:// push/pull)
    rw = Wisp(replica, w.actor, w.bin)
    w.run_allow_fail("remote", "remove", "replica")
    w.run("remote", "add", "replica", f"file://{replica.resolve()}", "--trusted")
    w.run("push", "replica")
    rw.run_allow_fail("remote", "remove", "origin")
    rw.run(
        "remote",
        "add",
        "origin",
        f"file://{demo.resolve()}",
        "--trusted",
    )
    rw.run("pull", "origin")
    rb = rw.run("search", "max pain", "--mode", "keyword")
    top_paths = [r["path"] for r in rb["data"].get("results", [])[:3]]
    print("replica top paths:", top_paths)
    print(
        "replica Fri observation still in index:",
        any("2026-10-16" in p for p in top_paths),
    )


def act4_audit(w: Wisp) -> None:
    step("ACT 4 — Auditor replay")
    print(w.run("fsck"))
    print(w.run("log", "-n", "5"))
    answer = answer_from_store(w, "What was the ex-post play on the Friday flush day?")
    print("\nGrounded Q&A (search + cat):\n", answer)


def answer_from_store(w: Wisp, question: str) -> str:
    hits = w.run("search", question, "--mode", "auto")["data"].get("results", [])[:3]
    chunks = []
    for h in hits:
        path = h["path"]
        body = w.run("cat", path, "--json")["data"]
        for s in body.get("sections", []):
            if s.get("heading") in ("", h.get("heading")) or h.get("heading") in s.get(
                "heading", ""
            ):
                chunks.append(f"### {path} — {s.get('heading')}\n{s.get('content', '')[:500]}")
    if not chunks:
        return "(no hits)"
    return "\n\n".join(chunks[:2])


def narrate() -> None:
    print(
        textwrap.dedent(
            """
            DEMO NARRATION (≈8 min)
            -------------------
            1. Two agents share one SPY desk bundle — not two RAG silos.
            2. Act 1: Hawk commits a live stub with --input to yesterday's observation (why/log).
            3. Act 2: Bear vs Hawk fight over Wed strategy; merge --resolutions picks a side; ledger keeps both histories.
            4. Act 3: Erase Friday max-pain section; search and replica pull mirror deletions.
            5. Act 4: fsck + log + grounded Q&A from section search — audit without chat logs.

            Run:  python3 demo/desk/desk_demo.py all --fresh
            """
        )
    )


def main() -> None:
    ap = argparse.ArgumentParser(description="Wisp agent desk demo")
    ap.add_argument("--source", type=Path, default=DEFAULT_SOURCE)
    ap.add_argument("--demo", type=Path, default=DEFAULT_DEMO)
    ap.add_argument("--replica", type=Path, default=DEFAULT_REPLICA)
    ap.add_argument(
        "--llm",
        action="store_true",
        help="Act1 draft via demo/.env or SCRYPTED_BEARER_TOKEN; OPENAI_API_KEY fallback",
    )
    ap.add_argument("--pause", type=float, default=0.0, help="Seconds between acts")
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name in ("seed", "act1", "act2", "act3", "act4", "narrate", "all"):
        sp = sub.add_parser(name)
        sp.add_argument(
            "--fresh",
            action="store_true",
            help="Reset demo + replica (seed / all)",
        )

    args = ap.parse_args()
    bin_path = wisp_bin()
    w = Wisp(args.demo, "human:tim", bin_path)

    if args.cmd == "narrate":
        narrate()
        return
    fresh = getattr(args, "fresh", False)
    if args.cmd == "seed":
        seed(args.source, args.demo, args.replica, fresh)
        return
    if args.cmd == "all":
        seed(args.source, args.demo, args.replica, fresh=True)
        for fn in (act1_live_stub, act2_merge_fight, act3_erase, act4_audit):
            if fn == act1_live_stub:
                fn(w, args.llm)
            elif fn == act3_erase:
                fn(w, args.demo, args.replica)
            else:
                fn(w)
            if args.pause:
                time.sleep(args.pause)
        narrate()
        return
    if args.cmd == "act1":
        act1_live_stub(w, args.llm)
    elif args.cmd == "act2":
        act2_merge_fight(w)
    elif args.cmd == "act3":
        act3_erase(w, args.demo, args.replica)
    elif args.cmd == "act4":
        act4_audit(w)


if __name__ == "__main__":
    main()

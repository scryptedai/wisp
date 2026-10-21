#!/usr/bin/env python3
"""Regenerate deck-slides.json copy via Scrypted AI Nova Pro (optional polish)."""

from __future__ import annotations

import json
import sys
from pathlib import Path

from scrypted_llm import bearer_token, complete_text

OUT = Path(__file__).resolve().parent / "deck-slides.json"
BRIEF_PATH = Path(__file__).resolve().parent / "README.md"


def main() -> None:
    if not bearer_token():
        sys.exit(
            "Set SCRYPTED_BEARER_TOKEN or copy demo/.env.example → demo/.env"
        )

    readme = BRIEF_PATH.read_text()
    user = (
        "Rewrite slide copy for the wisp agent desk demo. "
        "Keep wisp-specific terms (OKF, section, merge, erase, actors, fsck, why, --input). "
        "Do NOT confuse fsck/cat with UNIX cat or generic 'trading books'. "
        f"Source README:\n{readme[:6000]}\n"
        "Return JSON array: id, title, bullets[], speaker, optional commands[]."
    )
    text = complete_text(
        user,
        "Technical demo writer. JSON only, no markdown fences.",
        max_tokens=4500,
    )
    if not text:
        sys.exit("no text in API response")

    raw = text.strip()
    if raw.startswith("```"):
        raw = "\n".join(
            line for line in raw.splitlines() if not line.strip().startswith("```")
        )
    start, end = raw.find("["), raw.rfind("]")
    slides = json.loads(raw[start : end + 1])
    OUT.write_text(json.dumps(slides, indent=2) + "\n")
    print(f"Wrote {len(slides)} slides → {OUT}")


if __name__ == "__main__":
    main()

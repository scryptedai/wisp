"""Load Scrypted bearer token from env or demo/.env (never commit those files)."""

from __future__ import annotations

import json
import os
import time
import urllib.request
from pathlib import Path

API = "https://api.scrypted.ai"


def _load_demo_dotenv() -> None:
    """Set env vars from demo/.env if present (simple KEY=VALUE, no export)."""
    env_path = Path(__file__).resolve().parents[1] / ".env"
    if not env_path.is_file():
        return
    for line in env_path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, val = line.partition("=")
        key, val = key.strip(), val.strip().strip('"').strip("'")
        if key and key not in os.environ:
            os.environ[key] = val


def load_demo_env() -> None:
    """Load demo/.env into os.environ (gitignored)."""
    _load_demo_dotenv()


def bearer_token() -> str | None:
    load_demo_env()
    return os.environ.get("SCRYPTED_BEARER_TOKEN") or os.environ.get(
        "SCRYPTEDAI_BEARER_TOKEN"
    )


def complete_text(user_prompt: str, system_prompt: str, max_tokens: int = 800) -> str | None:
    token = bearer_token()
    if not token:
        return None
    body = {
        "user_prompt": user_prompt,
        "system_prompt": system_prompt,
        "model": "amazon.nova-pro-v1:0",
        "max_tokens": max_tokens,
        "temperature": 0.5,
        "auto_calculate_tokens": False,
    }
    req = urllib.request.Request(
        f"{API}/generations/text/nova-pro",
        data=json.dumps(body).encode(),
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
        },
    )
    with urllib.request.urlopen(req, timeout=60) as resp:
        started = json.load(resp)
    if isinstance(started.get("result_data"), dict):
        text = started["result_data"].get("text")
        if text:
            return text
    job_id = started.get("job_id")
    if not job_id:
        return None
    for _ in range(45):
        time.sleep(2)
        poll = urllib.request.Request(
            f"{API}/jobs/{job_id}",
            headers={"Authorization": f"Bearer {token}"},
        )
        with urllib.request.urlopen(poll, timeout=60) as resp:
            st = json.load(resp)
        rd = st.get("result_data") or st.get("result")
        if isinstance(rd, dict) and rd.get("text"):
            return rd["text"]
        if (st.get("status") or "").lower() in ("failed", "cancelled", "error"):
            break
    return None

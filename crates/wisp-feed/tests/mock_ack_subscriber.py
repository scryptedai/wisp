#!/usr/bin/env python3
"""Minimal external subscriber: echo {"ack": <seq>} for each JSON line on stdin."""
import json
import sys

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    ev = json.loads(line)
    print(json.dumps({"ack": ev["seq"]}), flush=True)

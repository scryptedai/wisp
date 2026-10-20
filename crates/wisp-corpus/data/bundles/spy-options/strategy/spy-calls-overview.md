---
type: Note
title: Options Trading on SPY Calls — News Signal Playbook
description: How we trade short-dated SPY calls when macro and headline flow align.
tags: [spy, calls, news, discretionary]
status: draft
---
Living document for the desk. See [signal rubric](../signals/news-signal-rubric.md) and [position limits](../risk/position-limits.md).

## Thesis

We buy **0–2 DTE ATM or slightly OTM SPY calls** when a **high-impact news impulse** shifts the implied path of the index higher, but **before** the move is fully priced in options. We are not forecasting every headline; we trade only when the rubric score clears the desk threshold.

## Entry rules

- **VIX** below 22 at entry (no chase in panic vol).
- **News tier**: Tier-1 macro (CPI, FOMC, NFP) or Tier-1 single-name guidance that moves SPY beta names.
- **Structure**: Prefer calls with delta 0.35–0.45; max premium at risk per [position limits](../risk/position-limits.md).
- **No entry** inside 15 minutes of a known Tier-1 print unless the plan was written the prior session.

## Exit rules

- Take **50%** off at +40% premium; trail the remainder with a **-25%** stop from peak mark.
- Flatten all SPY call exposure **15 minutes before** the next Tier-1 event unless explicitly carried with written rationale.
- If the **news thesis is invalidated** (see observations), exit at market — do not average down.

## Risk limits (summary)

Desk-wide caps are authoritative in `risk/position-limits.md`. This section is a reminder only.

## Changelog (human)

Desk edits here; agents use `wisp update` on sections and cite observations as inputs.

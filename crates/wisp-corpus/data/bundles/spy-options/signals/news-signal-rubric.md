---
type: Note
title: News signal rubric for SPY call entries
description: Score headline flow before enabling discretionary call buys.
---
Use this rubric before any SPY call entry. Observations in `observations/` should cite sources.

## Tier definitions

| Tier | Examples | Typical SPY move if surprise |
|------|----------|------------------------------|
| 1 | CPI, Core PCE, FOMC statement, NFP | 0.8–2.5% same session |
| 2 | Mega-cap guidance, oil shock headlines | 0.3–1.0% |
| 3 | Single-stock M&A, sector fluff | Usually ignore for SPY calls |

## Scoring

Add points when true at decision time:

- **+3** Tier-1 print **below** consensus on inflation (dovish) with SPY up >0.4% in first 30m.
- **+2** Fed speaker surprise dovish vs OIS path.
- **+2** VIX down >5% from prior close while SPY green.
- **-3** Tier-1 **above** consensus inflation (hawkish) — **no new call buys** for the session.
- **-2** Headline explicitly contradicts an open thesis in [spy-calls-overview](../strategy/spy-calls-overview.md).

**Threshold**: score ≥ 4 to enable entries; 2–3 = half size only.

## Honest mistakes we catch

- Trading a **stale** headline already in the prior day's close (check observation timestamps).
- Double-counting the same wire story under two IDs.
- Confusing **pre-market** equity reaction with **bond market** signal (use both in observation).

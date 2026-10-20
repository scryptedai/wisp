---
type: Note
title: SPY options position limits
description: Hard caps; agents may not raise limits without human verify.
---
## Per trader

- Max **$25k** premium at risk in SPY calls per session.
- Max **3** concurrent structures.

## Desk aggregate

- Max **$80k** SPY call premium across the desk.
- **No** new entries after **15:30** ET unless `human:*` verify on the concept.

## Agent policy

Processes may **read** this file and **propose** edits in `strategy/spy-calls-overview` risk summary; only humans change limits here.

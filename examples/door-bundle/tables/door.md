---
type: BigQuery Table
title: Door Events
description: Events emitted by building doors.
tags: [interactive]
---
Events emitted by doors. See [sensors](../observations/2026-10-17.md).

## Schema

| col | type |
|---|---|
| door_id | STRING |
| opened_at | TIMESTAMP |

## Behavior

Doors open when pushed.

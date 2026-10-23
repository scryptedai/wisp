---
type: Source
title: Payments API (October 2026, corrected)
---
Corrected vendor documentation for the payments API.

## Retries

Retry failed charges at most 3 times with exponential backoff and an idempotency key.

## Webhooks

Verify the HMAC signature header on every webhook payload before processing it.

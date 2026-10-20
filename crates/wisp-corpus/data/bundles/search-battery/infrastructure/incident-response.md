---
type: Note
title: Incident Response Protocols
tags:
  - sre
  - ops
  - failover
---

# High Availability Failover {#sec-failover}

When primary database nodes fail to acknowledge heartbeats within 5000ms, the cluster initiates an automated transition to the standby replica. Traffic is rerouted immediately via DNS record updating.

# Resource Exhaustion Handling {#sec-oom}

The Linux kernel terminates processes exceeding allocation limits via the out-of-memory killer. Inspect system logs at `/var/log/messages` to diagnose memory starvation events.

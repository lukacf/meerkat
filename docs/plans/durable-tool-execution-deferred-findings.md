---
title: "Durable Tool Execution Deferred Findings"
description: "Earlier deferred findings from the durable shell and job execution delivery slices."
icon: "list-check"
---

# Durable Tool Execution Deferred Findings

This ledger records review findings that do not violate the active slice
contract. Recovery and lifecycle correctness are not deferred here.

| Slice found | Finding | Disposition |
| --- | --- | --- |
| Phase 3A | SQLite origin lookup filters `origin_session_id` after decoding realm rows because the v1 table has no dedicated session column. | Defer the indexed schema optimization to a later jobs-store migration; current results are complete and recovery is unbounded. |
| Phase 3A | A crash after job submission but before the caller receives the receipt leaves a queued shell job dependent on idempotent call replay; there is no general orphan-queue worker. | Defer a general queue consumer to the multi-runner worker surface. Phase 3A proves replay claims the same committed job exactly once. |
| Phase 3A | Raw shell commands are durable runner configuration and may contain user-inlined credentials. | Phase 4 must add typed credential references and execution-time resolution before credential-bearing callback acceptance; do not claim secret-safe detached callbacks before then. |
| Phase 3A gate | `meerkat-mob::runtime::tests::test_shutdown_does_not_stall_on_stuck_lifecycle_notification` transiently observed an unrelated unregister teardown still in progress during the full workspace run, then passed immediately in isolation. | Record as a pre-existing Mob teardown flake; do not expand the durable-shell slice. |

# job_runtime_delivery Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `job_runtime_delivery`

### Code Anchors
- `job_outbox_projector` (route `job_terminal_enters_runtime_inbox`): `meerkat/src/job_delivery.rs` — mechanical job outbox projector submits stable delivery identity into runtime-owned durable authority before acknowledging the job
- `job_notification_outbox_projector` (route `job_notification_enters_runtime_inbox`): `meerkat/src/job_delivery.rs` — mechanical notification outbox projection uses a job-scoped stable runtime delivery identity
- `runtime_delivery_inbox` (route `runtime_delivery_commit_acknowledges_job_outbox`): `meerkat-runtime/src/delivery_inbox.rs` — generated runtime delivery commit and exact reuse provide the only acknowledgements accepted by the job projector
- `job_runtime_delivery_schema` (route `runtime_delivery_reuse_acknowledges_job_outbox`): `meerkat-machine-schema/src/catalog/compositions.rs` — formal enqueued two-store job and runtime delivery composition

### Scenarios
- `runtime-delivery-first-commit` — terminal job outbox commit enters runtime delivery authority and is acknowledged only after the durable runtime insert
- `runtime-delivery-notification-commit` — nonterminal job notification enters runtime delivery authority and is acknowledged only after the durable runtime insert
- `runtime-delivery-crash-retry-reuse` — crash after runtime insert but before job acknowledgement reuses the original runtime sequence and then acknowledges once

### Routes
- `job_terminal_enters_runtime_inbox`
  - anchors: `job_outbox_projector`
  - scenarios: `runtime-delivery-first-commit`, `runtime-delivery-crash-retry-reuse`
- `job_notification_enters_runtime_inbox`
  - anchors: `job_notification_outbox_projector`
  - scenarios: `runtime-delivery-notification-commit`
- `runtime_delivery_commit_acknowledges_job_outbox`
  - anchors: `runtime_delivery_inbox`
  - scenarios: `runtime-delivery-first-commit`, `runtime-delivery-notification-commit`
- `runtime_delivery_reuse_acknowledges_job_outbox`
  - anchors: `runtime_delivery_inbox`
  - scenarios: `runtime-delivery-crash-retry-reuse`

### Scheduler Rules
- `(none)`

### Invariants
- `job_terminal_reaches_runtime_delivery_authority`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `job_notification_reaches_runtime_delivery_authority`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `runtime_commit_ack_is_the_job_delivery_ack_source`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `runtime_reuse_ack_is_the_retry_ack_source`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)


<!-- GENERATED_COVERAGE_END -->

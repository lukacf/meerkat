# job_runtime_delivery

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `job`: `DetachedJobMachine` @ actor `job_authority`
- `runtime_delivery`: `RuntimeDeliveryMachine` @ actor `runtime_delivery_authority`

## Routes
- `job_terminal_enters_runtime_inbox`: `job`.`TerminalCommitted` -> `runtime_delivery`.`CommitDelivery` [Enqueue]
- `runtime_delivery_commit_acknowledges_job_outbox`: `runtime_delivery`.`DeliveryCommitted` -> `job`.`MarkDeliveryApplied` [Enqueue]
- `runtime_delivery_reuse_acknowledges_job_outbox`: `runtime_delivery`.`DeliveryReused` -> `job`.`MarkDeliveryApplied` [Enqueue]

## Target Selectors
- `(none)`

## Driver
- `(none)`

## Transaction Plans
- `job_terminal_result_and_outbox` via `terminalize_job` / `DetachedJobStore::compare_and_swap` — job terminal result and pending outbox row commit atomically under generated job authority
- `runtime_delivery_identity_and_sequence` via `commit_runtime_delivery` / `RuntimeStore::compare_and_swap_runtime_delivery_authority` — runtime delivery identity, generated sequence authority, and inbox row commit atomically
- `job_outbox_acknowledgement` via `acknowledge_runtime_delivery` / `DetachedJobStore::compare_and_swap` — job outbox acknowledgement commits only after the runtime insert or exact replay succeeds

## Scheduler Rules
- `(none)`

## Structural Requirements
- `job_terminal_reaches_runtime_delivery_authority` — job terminal outbox entries enter generated runtime delivery authority before any acknowledgement

## Behavioral Invariants
- `runtime_commit_ack_is_the_job_delivery_ack_source` — a fresh job outbox acknowledgement originates from the generated runtime delivery commit effect
- `runtime_reuse_ack_is_the_retry_ack_source` — a replayed projector acknowledgement originates from exact generated runtime delivery reuse

## Coverage
### Code Anchors
- `job_outbox_projector` (route `job_terminal_enters_runtime_inbox`): `meerkat/src/job_delivery.rs` — mechanical job outbox projector submits stable delivery identity into runtime-owned durable authority before acknowledging the job
- `runtime_delivery_inbox` (route `runtime_delivery_commit_acknowledges_job_outbox`): `meerkat-runtime/src/delivery_inbox.rs` — generated runtime delivery commit and exact reuse provide the only acknowledgements accepted by the job projector
- `job_runtime_delivery_schema` (route `runtime_delivery_reuse_acknowledges_job_outbox`): `meerkat-machine-schema/src/catalog/compositions.rs` — formal enqueued two-store job and runtime delivery composition

### Scenarios
- `runtime-delivery-first-commit` — terminal job outbox commit enters runtime delivery authority and is acknowledged only after the durable runtime insert
- `runtime-delivery-crash-retry-reuse` — crash after runtime insert but before job acknowledgement reuses the original runtime sequence and then acknowledges once

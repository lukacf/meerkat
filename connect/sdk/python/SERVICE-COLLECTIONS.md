# Private protocol 7 service collections

This opt-in client requires an application-host-selected launcher embedding Connect's `run_with_service_collections` and an actual `ServiceCollectionSelection`. Ordinary `--stdio`, a browser request or an SDK constructor cannot supply that authority. The launcher must connect logout and expiry to the retained native control. Protocol 5 and protocol 6 clients remain separate and unchanged.

The host chooses source definitions, requirement/resource associations, operation ceilings and result-dependent programs before launching. The client sends only the observed `job_id` and `source_id` in `connect/collect`. It never sends a program, raw operation, argument, predicate, credential, grant, continuation or claimed completion. There is no downgrade, source replay, automatic retry or replacement process.

`state.collection_sources` lists sources across every service in the invocation. In multi-service mode, choose sources from the host's retained per-service composition for the observed `state.service_id`, wait for `state.can_collect`, and use that service's newly observed native `job_id`. The source list carries no source-to-service mapping. Native still checks membership before admission.

The existing protected setup flow can close its input owner with `unavailable(reason=finalizing)` while a distinct commissioning approval remains pending. That exact event does not retire the whole binding. Setup send and connector operations remain fenced. Other retirement, Close, cancellation and expiry remain permanent. Finalization, commissioning and collection availability are not evidence of source capability.

An optional native `first_fence` records the original transition as a historical diagnostic. The client checks its shape and scope and retains it unchanged across service phases. Unknown origin labels are preserved as nonempty, control-free text of at most 256 UTF-8 bytes; they do not enable collection or confirm cleanup. Any supplied diagnostic is checked on later retirement events too, including expiry and cancellation. Older peers may omit the diagnostic.

A returned collection report is a private historical native observation. It retains each requirement, resource, ordered call, argument digest and nested native input/run/tool-call/terminal/close facts. `historical_checks_completed` only describes this attempt. `reusable_source_coverage` remains `unqualified_credential_continuity`, including after completed checks. The SDK does not evaluate native result predicates, mint Ready, compare secret generations or authorize future ingestion. Empty, foreign, malformed and internally contradictory completed reports fail closed. Partial failure reports retain reserved-call and observed-over-budget counts. Unknown effects and cleanup failures are never rewritten as success or retried.

Collection selection is burned when queued. A matching `invalid_binding` refusal before any active collection observation keeps that job/source pair blocked, but permits an explicit selection of the source under a later native installation job. Native checks source membership before admitting a collection; the invocation-wide source list alone does not identify its service phase. Other refusals, observed admission and completed attempts keep the source blocked across phases. No selection is automatically retried, and the next native owner still decides admission. Cancel accepts only the active or latest completed collection ID while no next-service transition is queued. Queuing the next service ends the previous collection's cancel window immediately, even before the next native state arrives. Use `close()` to abort a queued transition and keep the same owner until it joins. Stale IDs do not target a newer collection. A consumed setup-send result is not retained as a new SDK cancellation handle; Close or an approval refusal can retire commissioning. Cancellation is not process reaping, native resource cleanup or proof of zero effects.

Keep events, reports, protected prompts and answers out of model messages, public RPC, telemetry and ordinary logs. Report access is explicit and reports remain immutable. The failure channel is value-free. The process owner is retained across observation/close timeouts and post-spawn startup failures. Store that owner in a long-lived host registry before awaiting close, and retain it on timeout; observe and join the same process again. Do not retry the collection or launch a replacement. A process exit without the exact native close receipt is not native cleanup confirmation.

## Validation and packaging boundary

The SDK includes synthetic-peer controls using real subprocess pipes. These check transport and attribution only, not native Ready, commissioned credentials, real collection programs, installed resources or providers. The reviewed native v7 source, a trusted launcher, installed positive/negative acceptance and final coherent Core/Meta/resource artifacts remain independent prerequisites. Existing v6 results are historical results for their exact original source; they are not v7 acceptance.

Import `ServiceCollectionClient`, `ServiceCollectionLauncher` and `CollectionStatus` from `meerkat_toolkit.service_collection`. Use the existing `ServiceSetupBinding`. The launcher snapshots its command, environment and credential policy. `client.collect(job_id=observed_job, source_id=observed_source)` returns a request ID. `client.next_event()` yields a `ServiceCollectionEvent`; only the actual matching result carries its typed `report`. `client.state` is the latest immutable projection, and `viewer_current_at_observation` is a historical local snapshot, not permission.

```python
from meerkat_toolkit.service_collection import ServiceCollectionClient
from meerkat_toolkit.service_setup import ServiceSetupLaunchError

try:
    owner = ServiceCollectionClient(host_selected_launcher, authenticated_binding)
except ServiceSetupLaunchError as failure:
    owners[host_request_id] = failure.owner
    raise
owners[host_request_id] = owner
# Render actual protected events. After native availability, submit only its IDs.
request_id = owner.collect(job_id=observed_job, source_id=observed_source)
# On retirement, keep owners[host_request_id] while close is pending.
receipt = owner.close(timeout=30)
if receipt.process_joined:
    del owners[host_request_id]
```

A joined process with `native_resources_closed=False` still requires the host to retain the failed outcome. This example does not manufacture observed IDs or a selected launcher.

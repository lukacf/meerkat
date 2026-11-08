# Private service probes for trusted hosts

`@rkat/toolkit/service-probe` consumes the native protocol6 adapter. It keeps setup, protected commissioning, finalization, governed reopening and required-operation observations on one retained process. Native code owns authentication, grants, dispatch, durable completion and cleanup. The SDK validates the wire contract and attributes observations to the selected host binding.

This client requires an application-owned executable that constructs `ServiceProbeSelection` and calls `meerkat_connect::stdio::run_with_service_probes`. The ordinary `meerkat-connect --stdio` entrypoint does not support protocol6. A pathname or successful subprocess start does not prove the embedding is correct. A successful initialization checks its wire protocol, not live service access. There is no automatic downgrade or fallback.

## Select the launcher in trusted host code

```ts
import { ServiceProbeClient } from "@rkat/toolkit/service-probe";
import type { ServiceProbeLauncher, ServiceSetupBinding } from "@rkat/toolkit/service-probe";

// Supplied by the authenticated application's existing native integration.
declare const binding: ServiceSetupBinding;
declare const launcher: ServiceProbeLauncher;
const owner = new ServiceProbeClient(launcher, binding);
```

The launcher contains `protocol_version: 6`, an absolute `executable`, exact `args`, a `credential_scope` equal to the binding's selected policy, and an optional restricted `environment`. The SDK copies these before spawning. It adds no flags, reads no ambient provider secrets, and does not create a native profile from launcher arguments. Native-default credentials require an explicitly selected absolute `HOME`; file custody excludes ambient home discovery. See [Services setup](SERVICE-SETUP.md) for the binding and protected answer vocabulary.

The actual embedding must retain `ServiceProbeControl` and connect it to real host logout and selection invalidation. An immutable configuration file cannot observe logout. Native caller identity, ingress, operation profiles, resource selectors, result contracts and grants never come from a browser probe request.

## Consume the real lifecycle

Initialization can emit state and provider prompts before its final session result. The client validates their initialization target. After successful setup, native emits `unavailable` with reason `finalizing` when the setup-input owner closes. This retires setup messages and connector commands. The separate commissioning phase can still offer protected native approval prompts under the same viewer and deadline.

`service_commissioning` preserves actual governance, controller/catalog issuance and disposal observations. Its `capability_verified` is always false. Saved administrative effects can remain after cancellation. In particular, the native controller grant has unrestricted lifetime and its protected review says it remains until explicitly revoked; logout fences the private caller without claiming that saved grant was revoked.

Finalization and a `service_probe_state` observation precede probe availability. The initial native session ID remains the correlation identity throughout. `owner.state` is the last immutable native projection; `can_probe` is an operation affordance, not a successful resource check.

```ts
// These identifiers must come from this owner's observed native job/profile list.
declare const observedJobId: string;
declare const observedProfileId: string;
const requestId = owner.probe({ job_id: observedJobId, profile_id: observedProfileId });

// The host continues its single private event loop, handling offered prompts.
declare const event: Awaited<ReturnType<typeof owner.nextEvent>>;
if (event.request_id === requestId) {
  const report = owner.probeReport(event);
  if (report) {
    // Private native facts for this one attempt. No automatic Build admission.
    const currentAtObservation = owner.viewerCurrentAtObservation(event);
    const currentNow = owner.viewerCurrent;
    void [report, currentAtObservation, currentNow];
  }
}
```

`probe()` sends only `scope`, `job_id` and `profile_id`. There is one active probe and one attempt per selected profile, including refusals and unknown outcomes. It does not accept a tool name, arguments, endpoint, resource object or new profile. `probeReport(event)` and `commissioning(event)` return deeply frozen typed observations only for events validated by that same client. A constructed or foreign event cannot supply a report. Event JSON and inspection omit private payloads; accessing a payload or typed report explicitly exposes it to host code, which must keep it out of model messages, public RPC, telemetry and ordinary logs.

## Interpret outcomes without promoting them

| Observation | Meaning |
| --- | --- |
| Administrative finalization or commissioning completed | Native setup effects were observed. Required resource access is still separate. |
| `tool_completed` | Exact native operation/result correlation, committed input and joined close were observed. Resource semantics remain unverified. |
| `capability_verified` | The selected native result contract verified this exact operation/resource observation. It is not a reusable grant or proof for another resource. |
| `outcome_unknown` | The actual effect or terminal outcome is unknown. No automatic replay. |
| `tool_failed`, refusal, unsupported or cleanup failure | Preserve the distinct native result. None establishes successful required access. |

The report retains nullable input/run/tool IDs, installed-connection and result hashes, physical `dispatch_outcome`, native terminal commitment and close facts. An `unknown` dispatch cannot become known failure or success. Local cancellation or expiry does not erase previously observed native facts. `viewerCurrentAtObservation` is a historical local snapshot; `viewerCurrent` reflects the client's current transport/lifetime observation. Neither replaces native caller admission or the host's live authentication state.

## Cancel and close the same owner

`cancel(requestId)` accepts the active probe or the latest completed probe if no newer probe is active. It permanently retires the binding. An old request cannot cancel a newer probe. The returned ID acknowledges local queuing only; wait for the native correlated result and call `close()` to join the actual process. An observation or close timeout retains that same owner. Continue observing or closing it; never restart or resend on that basis. `ServiceSetupLaunchError.owner` also retains an already-spawned process if startup fails after acquisition.

`close()` requires the native close receipt and a successful child exit without transport failure before reporting `native_resources_closed`. That receipt does not establish resource capability.

## Integration scope

The current native primitive supports up to sixteen host-selected, fixed, one-shot operation profiles. It does not implement arbitrary PR enumeration, pagination, history ingestion or head rechecks. A tool-name grant does not enforce repository argument isolation for unrelated later inputs. The exact probe gate narrows its own call arguments; the result contract also checks the actual installed target. Broader application resource enforcement and result-dependent ingestion remain separate native integration work.

The trusted launcher, exact provider artifact/profile selection and real positive native/provider acceptance must be qualified together. Synthetic subprocess tests establish SDK transport behavior only. Hosted Slack fixed-client MCP support remains unavailable in the currently selected source pair; its proposed release inclusion is conditional until exact reviewed source and qualification are delivered. Slack incoming conversation needs its own receiver, verification, admission, deduplication and reply audience integration.

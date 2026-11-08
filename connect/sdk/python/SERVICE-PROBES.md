# Private Services protocol 6 client

This source proposal adds `meerkat_toolkit.service_probe.ServiceProbeClient`.
It reuses the protocol 5 private process owner. Protocol 5 callers, wire and
existing 18 transport controls remain selected unchanged.

The host must explicitly select a `ServiceProbeLauncher(command,
credential_scope, environment)`. The exact absolute command is executed without
extra flags, a shell or ambient environment. Ordinary `meerkat-connect --stdio`
does not implement protocol 6. A trusted embedding must construct the native
`ServiceProbeSelection` and invoke `run_with_service_probes`; this SDK does not
implement that embedding or treat a filename as proof. The handshake requires
protocol 6 and the native probe capabilities. There is no downgrade or replay.

The immutable launcher copies argv, credential policy and environment. The
credential policy must match `ServiceSetupBinding.credential_scope`. File
credentials allow only the existing minimal process environment; native-default
requires the host's explicit fixed absolute HOME. The launcher itself must use
that selected native persistence policy. No credential root, resource selector,
operation, grant, profile body or continuation is transmitted in initialization
or a probe request. Initialization retains the existing closed application,
viewer/login/deadline, realm, adapter and service preparation vocabulary.

Read `next_event()` to present actual native setup prompts and observations.
The ordinary closed `answer`, `submit`, and connector methods are inherited.
Only the exact initialized same-scope `unavailable(reason=finalizing)` retires
the setup input/connector owner without retiring the whole v6 binding. It never
clears an earlier cancellation, expiry or other unavailability. Protocol 5
continues to retire on every unavailable event. After setup closes, `available` and `can_send` can be false while commissioning
is active or `can_probe` becomes true. This does not mean the child or whole
binding is retired. A native commissioning approval remains actionable under
its actual prompt ID and current viewer. The SDK does not approve it itself.
Saved administrative effects in `service_commissioning` remain observations even
if later work fails. Closing or cancelling does not revoke saved grants.

A `service_probe_state` event supplies the actual job ID; `probe_profiles` lists
opaque host-selected IDs. Call `probe(job_id=..., profile_id=...)` with exactly
those two observed selectors. The returned request ID is only correlation.
Read its `ServiceProbeEvent.report` for the frozen `ServiceProbeReport`.
Scope, request, job, profile, application, viewer/login, realm and original
native session are checked before delivery. Server, operation and resource are
native observations selected by the embedding; the SDK cannot create or verify
that authority. Report status and nullable `dispatch_outcome` remain separate.

`tool_completed` proves neither resource semantics nor capability verification.
Only native `capability_verified` reports with committed dispatch, exact native
input/run/call and hashes, terminal commit and joined native close pass the
positive report checks. Unknown, failed, unsupported, cancelled and cleanup
unconfirmed stay distinct. `viewer_current_at_observation` is a local snapshot,
not future authorization. A report arriving after local retirement preserves
native facts with that flag false. Never turn finalization, commissioning,
`can_probe`, or a successful process exit into capability verification.

`cancel(request_id)` accepts an active or latest completed probe and permanently
retires local admission even if its bounded outbound queue refuses the cancel.
Older or foreign IDs cannot retire a different operation. The native owner
independently checks correlation. For actual host logout, the embedding must
retire its `ServiceProbeControl`, and the host must retain the SDK owner through
`close()` until the actual child and reader/writer join. A timeout retains the
same owner; it does not kill, replace, retry or authorize another attempt.
Keep a `ServiceSetupLaunchError.owner` in a long-lived host registry before
attempting close. If that observation times out, keep the owner registered.

## Validation and remaining prerequisites

The proposal adds actual-subprocess wire test source for protocol negotiation,
early prompts, post-setup commissioning approvals, exact report attribution,
unknown effects, cancellation, backpressure, expiry/logout, EOF and joined close.
These controls are unrun and use explicit fake peers, not native providers.
The 18 existing protocol 5 controls remain required. There is no new native,
provider, resource, published SDK or end-to-end acceptance result.

Required before product acceptance: exact reviewed native v6 source and its
implementing dependency tuple; real trusted launcher wiring and logout control;
reviewed commissioning/controller/catalog custody; positive native continuation
and result correlation; actual required resource semantic checks; and installed
SDK/package validation. Fresh-host commissioning remains in the intended scope.
Generic hosted Slack fixed-client MCP OAuth and Slack inbound remain separately
unsupported; this transport does not add either capability.

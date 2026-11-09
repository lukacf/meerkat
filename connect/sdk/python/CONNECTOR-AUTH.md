# Generic connector sign-in on an existing host step

`meerkat_toolkit.connector_auth` implements bounded callback transport for the
public Python connector OAuth methods. `connector_auth_host` connects it to an
existing authenticated host step and the current Toolkit UI projection. The
embedding application supplies admission, the step's lifetime, and a protected
human presenter. No new agent runtime, endpoint, credential store or setup state
machine is introduced.

The adapter's retained execution evidence used PR 1732's earlier Python SDK
with a scripted JSON-RPC peer, not a native server or provider. A 2026-10-08
source readback now binds final head `d945f2d4522798f59248763d4a1657236dc44fab`
and merge `983d1c846665da5c41eeda43a8ce1f69ea4936b6`. Their inspected connector
APIs are identical, including the Python start/complete/cancel/status methods
consumed here. This readback does not rerun the earlier tests or qualify native
integration. The baseline inspected on that date, Meerkat `0.8.51` / MobKit
`0.8.47`, lacks the merged connector login owner and its HostAuthService APIs.
Do not enable this path on that historical baseline. See the
[dated source map](../../docs/verification/20261008-connect-service-account/OWNER-MAP.md)
and the preserved [earlier consumer review](../../docs/coordination/20261005-connector-oauth-consumer/REVIEW.md).
See [ordinary CLI and Console setup](../../docs/site/guides/manage-connections.mdx#ordinary-cli-and-console-setup)
for Connect's broader acquisition journey. A completed service-acquisition case
does not establish a native OAuth provider composition for this adapter.

## Attach once to the host's existing step

The host chooses the storage slot and concrete target after admission. A slot is
a storage address, not an account. `Discover()` requests account discovery for an
empty slot. `Known(subject)` requests a compatible reconnect of that account.
Native owns emptiness, compatibility, account verification, credential admission,
refresh and scope evidence. A refused Discover must not become Known silently.

```python
from meerkat_toolkit.connector_auth import ConnectorSlot, ConnectorTarget, Discover
from meerkat_toolkit.connector_auth_host import ConnectorSignInBinding, HostStepReference

def attach_sign_in(client, *, realm_id, slot_id, issuer, client_id, resource,
                   scopes, strategy_id, step_reference, revalidate,
                   protected_presenter=None):
    # The caller supplies an already admitted target and existing host reference.
    return ConnectorSignInBinding(
        client,
        ConnectorTarget(ConnectorSlot(realm_id, slot_id), issuer, client_id,
                        resource, tuple(scopes), strategy_id, Discover()),
        reference=HostStepReference(**step_reference),
        revalidate=revalidate,
        protected_presenter=protected_presenter,
    )
```

Retain the binding on that step across requests and UI mounts. Do not construct
one per click, reconnect it to a different person, or expose it as an agent tool.
Its custody is process-local; it is not durable replay protection or restart
recovery. Existing host operation records remain authoritative after a process
loss. Do not reconstruct a binding and blindly start again after an uncertain
outcome.

`revalidate(context, operation, request, target)` must authenticate the current
person and check the exact target, scope, host reference/revision, current offer,
permission and intended effect. It raises on denied or stale admission. For
reads, `request` is `None`; the authenticated step and target must still be
checked. For a projected offer, the callback checks whether that viewer may see
and exercise it. This callback has no permissive default. Parsing a control or
matching a reference is not authorization.

`protected_presenter=None` disables opening and omits the sign-in control from
the projection. The host authenticates the person, revalidates the exact offer
and destination, and keeps auth-page content, credentials, state and callback
material out of model/tool observations and diagnostics. A supplied function,
new window or successful browser launch does not prove those properties.
Connect permits a host administered by agents; it requires no separate inaccessible
machine or protected-service placement. The presenter uses that host's supported
browser or system ceremony.

## Use the existing dispatcher and projection

Pass the current UI's `ControlRequest` directly to
`await binding.dispatch(authenticated_context, request)`. It freezes the
displayed scope, view revision, offer and target before any await. Supported
controls are `open_authorization`, `reconnect` for Known targets, and `stop_setup`
when the host admits stopping this exact sign-in transport. Whole-task
cancellation, account changes, disconnect and grant revocation belong to their
existing owners and are rejected here.

Opening performs admission, starts or retrieves the retained callback transport,
revalidates before presentation, and passes the private URL only to the host's
presenter. The reply acknowledges presentation only. It never says that the
account, connection, grant or original task is ready. While the attempt is active,
reopening an admitted offer can present the same retained URL; it does not create
another native start. After completion, closure or uncertainty, a later ceremony
requires a new decision by the existing host lifecycle owner. Another click or
rebinding the target cannot restart this custodian.
Unlike MCP login, connector start has no Started/Joined disposition. Connector
targets, results and cleanup observations remain separate from MCP types.

The host inserts `await binding.project_action(...)` into the existing task's
`userActions` list. Required inputs are the action ID, integration label, explicit
host-owned action state, safe instructions and an optional currently offered
control. The binding produces the current `UserActionView` shape, including an
inert authorization-origin label once available. It never projects the full URL,
state, code, callback or native response, and never invents an expiry.

Supply `state` from the actual host action owner, not from a local timer, callback
arrival, presenter return or native `authorized` string. Supply a complete,
audience-filtered `ToolkitSnapshot` through the existing host projection route.
Do not put private values into its otherwise valid labels, instructions or
diagnostics. The DTO schema is not a secret detector.

The [UI composition example](../typescript/examples/connector-sign-in.mjs) takes
that snapshot and the host's existing authenticated dispatcher. It returns the
ordinary view's `update` and `dispose` operations. Disposal removes presentation;
it does not cancel a request, close the callback listener or stop setup. No new
browser authorization API or transport is required by the example.

## Wait and reconcile without guessing

`await binding.wait(context)` waits only for an already started callback. It
checks admission before waiting and again before returning the account/scope
observation. `await binding.observe(context)` performs one similarly admitted
native status read, with no refresh, polling, start or automatic retry. A caller
cancelled during start can later explicitly dispatch its still-permitted offer
to retrieve the retained start. A cancelled waiter or disposed panel leaves the
callback transport with its host custodian.

Start repeats the exact frozen target on completion, including tagged
`account_selection`, scopes, issuer, client, resource and strategy. Cancel/status
address the retained slot; cancel also sends the private state. Wrong-slot,
malformed, lost or otherwise uncertain replies do not authorize a new start,
callback replay, fallback account or successful cleanup claim. Errors are fixed
local diagnostics. They do not classify native refusal prose as SlotOccupied,
AccountMismatch or other native authority facts. Status cannot resolve an
unknown start's attempt ownership.

`ConnectorCredentialObservation` preserves the native verified issuer, strategy
and subject separately from the slot and requested account. Granted scopes and
native scope evidence remain separate from task grants. Missing status scopes
remain `None`; an explicitly returned empty list becomes `()`. Neither is copied
from requested scopes. Status may omit account and evidence, even when it reports
`authorized`; the adapter preserves that absence.

The final merged native owner has the status/lifecycle correction: status reads
through credential admission and separates reauthorization from an admitted
credential. It does not refresh or contact the provider. This adapter's
`credential_view_fields(observation)` remains an attention-only projection
pending actual host composition and qualification; it cannot produce ready, a
grant, activation or original-task completion. The host supplies connection
identity, audience, task references and actual grants independently.

`stop_setup` is valid only when the existing host owner has decided that no
surviving waiter needs this transport. It acknowledges callback closure and
native pending-attempt cancellation separately. It never revokes credentials or
rolls back a completed effect. Call `binding.shutdown()` only for explicit host
shutdown, not in an HTTP-request or panel-disposal `finally` block. Lost cancel
replies stay unconfirmed; a false native `cancelled` value is not success.

## Profile limits and qualification

This transport binds `127.0.0.1` on an ephemeral port with a bounded local HTTP
callback. It does not claim compatibility with Slack's fixed callback profile or
remote/GitOps hosts. Browser-to-host routing and provider redirect registration
must fit an explicitly supported profile before enabling it.

The merged Rust owner already provides private credential use with refresh via
`HostAuthService::connector_oauth_authority()?.bearer_token(&slot)`. It rechecks
the admitted account and stable context after a shared refresh. The Python SDK
methods consumed here return no bearer credential and supply no private service
request bridge.

Provider-specific ceremony remains a separate integration question. In the
2026-10-08 source snapshot, the high-level strategy supplies account verification
and optional native nonce custody; its discovered authorize/exchange/refresh
profile has no Google offline/consent or client-secret configuration hook, or
Slack-specific exchange
format hook. Lower-level native flow, descriptor verification and atomic commit
APIs exist, but a supported provider composition and refresh continuity must be
bound before use. The existing stateless Google helper is not automatically
plugged into that owner. That source review does not qualify a released native
provider composition or private same-account request use through the Services
borrowed HTTPX client. Qualify the selected provider and native revision together
before claiming that path works. No token export to the model, ambient account
fallback or second credential store is introduced.

Verification is in [the finite source evidence](../../docs/verification/20261005-connector-auth-source/).
It covers public SDK framing, target continuity, uncertainty, host admission,
callback custody and UI disposal. Synthetic presenters/admission and scripted
RPC peers are not evidence of protected human auth, native credential use,
provider compatibility, live account success or completion of the original task.

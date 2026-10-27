# MCP browser login for Python hosts

`meerkat_toolkit.mcp_auth` supplies callback transport for Python hosts using
Meerkat's existing MCP login API. A [recorded isolated Linux qualification](../../docs/verification/20261006-mcp-native-login/REPORT.md)
used the earlier loopback adapter and custodian with published Meerkat 0.8.51 RPC
and Python SDK artifacts, a synthetic issuer and `TOKIO_WORKER_THREADS=2`. The
original startup failure remains retained. That result does not qualify the later
registered-HTTPS additions or repin Toolkit dependencies. Real provider login,
protected browser and authenticated requester admission remain unverified.
These are the qualification limits of this callback adapter. See
[ordinary CLI and Console setup](../../docs/site/guides/manage-connections.mdx#ordinary-cli-and-console-setup)
for Connect's broader acquisition and original-task continuation journey. A
completed service-acquisition case does not qualify this MCP OAuth transport.

For generic service connector OAuth, see [CONNECTOR-AUTH.md](CONNECTOR-AUTH.md).
That adapter has separate slot/account target and result types, including
Known/Discover selection, and no MCP Started/Joined wire disposition. It is
separate from the MCP callback transport selection described here.

The existing `MeerkatClient` already supplies `auth_mcp_login_start`,
`auth_mcp_login_complete`, `auth_mcp_login_cancel` and `auth_mcp_status`.
Toolkit calls those methods; it adds no JSON-RPC protocol or credential store.
Native auth owns target admission, PKCE, one-time state, account/scope
verification, credential persistence and attempt lifecycle.

The Meerkat 0.8.51 build used in that qualification requires an HTTP loopback
callback. By default this adapter binds `127.0.0.1` on an ephemeral port before
start, accepts the callback, and joins its socket tasks on settlement. Rust
embedding hosts can instead use `HostAuthService::mcp_begin_loopback_login`.
Browser localhost must refer to the same host.

Current Toolkit source also accepts a host-owned registered-HTTPS callback
inbox using the existing SDK start/complete signatures. This does not make
HTTPS login available on released loopback-only native builds. The native
owner has accepted [#1808](https://github.com/lukacf/meerkat/issues/1808), recorded
in the [exact contract receipt](../../docs/coordination/20261006-paired-host-presenter/20261006T082420.307696-claude-gcp-lead-28b77d.json),
for per-target callback admission and client registration. That receipt records
contract acceptance, without qualifying a released-native HTTPS consumer.
This source adds no native configuration or
capability override, future attempt-status method, persisted recovery, HTTPS
server or protected presenter. Native support and actual host admission are
prerequisites to production use; an unsupported start is never retried through
another transport.

## Host prerequisites

Call this API only from authenticated host code. Resolve the exact configured
MCP server and stable `oauth_account` through that host, without guessing a
subject from an email/display name. Native start requires the configured
selection and an applicable account-verification strategy. Registration and
administrator prerequisites still belong to the selected integration. This
component does not turn an unprepared OAuth application into beginner-ready
setup or bootstrap an unknown account identity.

The host supplies an admitted human presenter and keeps privileged SDK objects,
auth-page content, callback data and secrets out of model/tool transcripts and
diagnostics. The presenter can use the ordinary host and its supported browser
or system ceremony, including a host administered by agents. An
agent-unreachable machine or separately inaccessible presenter is not a
prerequisite. The host still validates the person, exact action and destination;
a browser launch, cookie or boolean flag proves neither authentication nor
permission. This package supplies callback transport, not a browser opener or
host admission implementation.

The shared Toolkit UI's `open_authorization` control carries only an owner
reference. The authenticated host revalidates that reference, person, current
offer and target before entering this adapter. Nothing here issues such an
offer, authorizes a requester or decides whether a shared ceremony may stop.

## Retain transport custody under an existing host step

For request or panel handlers, use `McpLoginCustodian`. Construct and retain it
under the existing authenticated host step before awaiting login. It fixes the
client, exact target and I/O bounds and retains one start task and its result.
Cancelling a request waiter leaves that task available to a later admitted
request. Concurrent calls retrieve the same handle; a failed or uncertain start
is retained without an automatic retry.

```python
from meerkat_toolkit.mcp_auth import McpLoginCustodian, StartedMcpLogin

# During creation of an existing host-owned step, before the first await:
step.mcp_login = McpLoginCustodian(client, target)

# In each separately authenticated and admitted open request, including replay:
await revalidate_person_offer_and_target(step)
login = await step.mcp_login.begin()
if isinstance(login, StartedMcpLogin):
    await login.present(human_presenter)
```

`step`, admission and `human_presenter` are existing application-owned
composition points, not helpers or authorities supplied by this package. The
host must revalidate every invocation, including repeated opens and close. An
initial review concerns the configured MCP target and requested account. The
actual authorization URL arrives from native start; the human presenter
must review its actual destination before navigation. No model-visible URL or
generic browser opener is added by this API.

Use the returned handle's existing `wait()` for credential observation and
`observe_mcp_auth()` for an explicitly admitted native status read. Neither is
connection readiness, installation completion or permission to resume a task.
Do not put `close()` in panel-disposal or cancelled-request cleanup. Only the
host custodian's explicit shutdown calls `await step.mcp_login.close()`; retain
the object until that cleanup settles. Close joins a pending start and closes
only its own started transport. A joined attempt or a never-started object
does not trigger native cancellation. Close permanently refuses subsequent
`begin()` calls, including a pending caller whose result loses the race with
close. A failed start retains its original error and cleanup uncertainty.

This helper does not authenticate callers, issue offers, implement a queue,
store credentials or recover transport custody after process loss. A host
must reconcile uncertainty through the native owner; constructing another
object is not permission to retry. The isolated released-native qualification
exercises waiter cancellation/rejoin, native Started/Joined, one synthetic code
exchange, status and explicit owner cleanup. It does not establish a protected
browser, host authorization, process-loss recovery or a real provider login.

## Host-owned registered-HTTPS callback

Use `RegisteredHttpsCallback(target, redirect_uri)` inside the host-owned callback
handler for the existing admitted step and exact HTTPS route. The ordinary-host
release path does not require an agent-unreachable machine or a separate protected
service; callback material must still stay out of model/tool transcripts and diagnostics. Pass it as
`callback=` to `McpLoginCustodian` or `begin_mcp_login`. Omitting it preserves
the loopback default. The callback fixes the target and exact URI before any
start; native must independently admit the same selection. URI bytes are not
normalized. This initial handoff supports HTTPS URIs without a static query or
fragment; those forms are rejected before start rather than silently rewritten.

```python
from meerkat_toolkit.mcp_auth import RegisteredHttpsCallback, McpLoginCustodian

# The host already selected the target, URI and admitted step.
step.callback = RegisteredHttpsCallback(step.target, configured_https_uri)
step.mcp_login = McpLoginCustodian(client, step.target, callback=step.callback)

# Register this closure at that exact HTTPS route before beginning login.
# The host router verifies the real origin/path and resolves this admitted step.
async def receive_callback(raw_query: str) -> bool:
    return step.callback.receive(
        redirect_uri=configured_https_uri, query=raw_query)

# In a separately authenticated and admitted human open request:
await revalidate_person_offer_and_target(step)
login = await step.mcp_login.begin()
```

These host objects, route registration and admission checks are composition
points, not supplied helper functions or a new registry. The URI passed to
`receive` comes from verified route configuration, not callback fields or
untrusted forwarded headers. It is a consistency check, not proof of TLS or
human identity. TLS, routing, requester admission and human presentation and route removal
remain host responsibilities. Invoke `receive` on the custodian's event loop,
with the raw ASCII query before conversion to a dictionary. It applies the
same 8192-character, 32-field, strict UTF-8/percent-decoding and duplicate-key
checks as the loopback handoff. Provider extras never replace target, account
or scope. The host returns fixed nonsecret HTTP text; `True` means only local
receipt, not successful login.

Only a verified native `Started` response arms the inbox, before presentation.
Early, wrong-state, wrong-URI, duplicate and late events cannot complete it.
One instance can be claimed once, before the first await; a failed target or
ownership claim cannot retire the winning custodian's inbox. A valid `Joined`
response retires only the unused local inbox and supplies no presenter/cancel
authority. An invalid or lost start response retains uncertainty without a
guessed native cancel. The same existing waiter cancellation, explicit close
and one-completion behavior applies after `Started`.

Here `CleanupObservation.callback_closed` means the claimed local inbox was
retired and no longer accepts callbacks. It does not assert that the shared
HTTPS server, route or in-flight HTTP handlers stopped. A losing claim reports
`False` with `native_cancel="not_requested"`; it does not close another owner.
Closing before `begin()` likewise leaves a supplied, unclaimed inbox untouched
and reports `False`.
The host separately removes its own route when its step ends, retaining the
custodian through settlement. No callback server is stopped by this inbox.
There is no new attempt-status or restart implementation; credential status
cannot substitute for the future native attempt read.

## Low-level use

The host owns an already-connected, compatible Meerkat client. This module
does not launch, download or locate a native binary. No SDK release is installed
or pinned by this source slice.

```python
from meerkat_toolkit.mcp_auth import (
    JoinedMcpLogin, McpTarget, begin_mcp_login,
)

# These values and presenter come from authenticated host configuration.
target = McpTarget(server_name, configured_server_url, stable_account_id)
login = await begin_mcp_login(client, target)
if isinstance(login, JoinedMcpLogin):
    # Another custodian still owns the pending callback. Keep waiting through
    # that owner's existing status path; do not cancel it or bind its redirect.
    show_waiting_elsewhere(login.presentation)
else:
    try:
        # Only this host-owned human presenter receives the full authorize URL.
        opened = await login.present(human_presenter)
        if not opened:
            show_browser_launch_unconfirmed()
        credential = await login.wait()
        show_native_credential_observation(credential)
    finally:
        # The ceremony custodian closes on actual abandonment/host shutdown.
        # Do not put this finally block in a disposable panel or waiter.
        cleanup = await login.close()
        retain_cleanup_observation(cleanup)
```

The named host functions above are composition points, not provided helpers.
The host retains the `StartedMcpLogin` until `wait()` or `close()` settles.
Cancelling a `wait()` task does not cancel the ceremony. Repeated `close()`
calls join one cleanup request; caller cancellation is reported only after
that cleanup has settled locally. A `JoinedMcpLogin` has neither `close()` nor
`present()`, and its unused newly bound listener or callback inbox is retired.
Generic panel disposal must not call `close()`. Stopping one acquisition does
not authorize cancellation of surviving waiters' native attempt.

`present()` returning true proves only that the supplied callback returned.
False or cancellation leaves the ceremony running, without automatic retry,
cancel or new start. A later explicit presentation remains the same attempt.
The caller must still keep its custodian and eventual cleanup alive.

## Observations and failures

`AuthPresentation` contains a canonical destination origin and the requested
account label. The origin is not verified issuer identity, and the requested
label is not proof of actual account verification. Only the full URL passed to
the human presenter contains attempt material. Do not serialize private
adapter or SDK objects into snapshots, diagnostics, traces or tools.

Successful completion returns an allowlisted `CredentialObservation`. Its
account must match the selected native expected account: native descriptor
verification enforces that equality before persisting `account_id`. It is not
an email normalization or Toolkit identity mapping. Credential observation is
not MCP connectivity, a grant, installation readiness or business completion.
`observe_mcp_auth` performs one explicit status read, without refresh/polling.
Native `authorized` can include an expired credential with a refresh token.

Loopback callbacks use bounded HTTP/1.1 GET requests to the exact path and Host. The
adapter rejects duplicated query keys, invalid UTF-8/percent escapes, mismatched
state, conflicting code/error and replay. Extra provider fields are bounded
and discarded; they cannot supply account, issuer, scope or authority to the
completion call. Only native complete verifies OAuth facts. Callback responses
contain fixed text, no reflected data, no-store and no-referrer headers; a 202
means received, never authenticated. Provider denial retires the owned attempt
through native cancel, without forwarding its description into diagnostics.

`callback_timeout` and `rpc_timeout` are host I/O budgets, each positive and at
most 300 seconds. They are not native expiry or a retry permit. If the host
abandons its callback wait, it closes local I/O and asks native cancel once.
Closing sockets does not prove native cancellation. `CleanupObservation`
reports that separately as `acknowledged`, `unconfirmed`, `not_requested` or
`not_needed`. An acknowledged cancel is a native attempt observation; it does
not prove that a concurrent completion created no credential.

Errors expose fixed codes and cleanup facts, without exception content. A lost
start reply, malformed reply after start or cancellation of the low-level
`begin_mcp_login` call during start leaves native admission uncertain. Local I/O
closes, but no cancel is guessed without a validated returned target/state.
Cancelling a `McpLoginCustodian.begin()` waiter instead preserves the retained
start task and its callback. A lost completion or unconfirmed cancel is
also uncertain. There is no automatic start, code replay, retry or reconnect.
After restart the old callback is unavailable; the host must reconcile with
native ownership rather than infer success or construct a replacement attempt.

## Focused verification

Run the module's unittest file using an existing Python 3.11+ interpreter and
an explicit minimal environment. `PYTHONPATH` selects `sdk/python/src`.
`TOOLKIT_TEST_PUBLIC_SDK` selects the exact retained public SDK source directory
ending in `accepted-public-sdk/sdks/python`; it is not an installed SDK override
for an application. Its receipt binds every source and license file.

The framing test loads the actual public `MeerkatClient`, connects it to an
explicitly selected scripted JSON-RPC process, and exercises all four existing
methods with real local callback sockets. It does not substitute an invented
client method or launch native Meerkat. The other tests exercise malformed
callbacks, account/target separation, Started/Joined, failed presentation,
cancelled waiters, immediate closure, partial HTTP requests, repeated caller
cancellation and uncertain start/completion/cancel effects. All bearer markers
are synthetic. These checks establish this adapter's behavior and SDK framing,
not native/provider execution, actual human consent or browser isolation.

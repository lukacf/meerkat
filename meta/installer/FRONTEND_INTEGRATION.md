# Application-owned frontend setup

This is the current integration contract for an application built by Forge.
It describes the authenticated Console facade in
[console_auth.py](launchkit/templates_v2/console_auth.py) and the application
owner in [app.py](launchkit/templates_v2/app.py). An application must implement
its own component configuration and activation. Declaring the hook does not
create Slack, Telegram or another transport.

## Declare the owner

The object returned by `application.build(context)` may expose
`frontend_management(request)`. The hook may be synchronous or asynchronous.
The application must also declare local-password Console authentication and
its private setup fields. Forge binds the hook to the exact loaded application
and deployment source. Changed source refuses further commands; importing new
code does not silently update that binding.

The hook receives the frozen `FrontendManagementRequest` from `console_auth.py`:

| Field | Supplied by the Console facade |
| --- | --- |
| `operation` | `inventory`, `prepare`, `submit` or `status` |
| `principal` | The configured, authenticated human account |
| `host` | The current application ID, host ID and loaded source digest |
| `payload` | The operation-specific object described below |
| `approved` | True only for an exact, protected human submission |

This is an application hook, not an agent callback tool or a native grant.
The application owns supported component types, participants, routing,
configuration revisions, change identifiers and activation observations.

## Read the loaded frontend declaration

`GET /console/forge/frontends/descriptor` reads compatibility metadata from the
declared application owner without invoking `frontend_management`. It accepts
no query parameters, body, caller-selected host, field list or component.
The closed HTTP 200 object has exactly these fields:

| Field | Value |
| --- | --- |
| `protocol` | Integer `1` |
| `host` | The existing closed `{protocol, host_id, application_id, source_digest}` identity |
| `private_fields` | The exact declared private names from that same retained loaded source, as a string array |

The names include the original compiled environment and declared application
private fields. No values are read from private settings or returned. An empty
array is valid. Duplicate names, wrong types and extra host fields refuse the
read; clients must also reject duplicate JSON keys or unknown envelope fields.
The complete response is bounded to 256 KiB. Component inventory is not a
declaration source.

The read requires the existing human Console session, with no bearer-token
substitution. It checks loaded source and import custody before and after the
awaited read, as well as the original login and closing state. A failed check
returns no partial declaration. Success and descriptor-specific refusals use
`application/json`, `Cache-Control: no-store`, and no `Set-Cookie` mutation:

| Status | Exact JSON body |
| --- | --- |
| 401 | `{"error":"Console login required."}` |
| 409 | `{"detail":"The loaded application source changed. No frontend command was admitted."}` |
| 501 | `{"detail":"This application has not declared frontend management support."}` |
| 503 | `{"detail":"The application frontend owner is unavailable. No activation is claimed."}` |

The outer login-required response may omit the cache header; clients still
require JSON and no response cookie mutation. Invalid methods, query parameters
or bodies are refused separately. An older wrapper's 404 refuses descriptor
consumption, with no inventory fallback or guessed names.

The existing protected selection/review owns the application identity check.
Before configuration use or same-conversation continuation, re-read at the same
literal loopback origin and compare the complete selected host and private-name
set. Changed identity, source or declarations require a fresh protected
selection. Fresh metadata never restores spent permissions, approvals or runtime
ownership, and it does not establish activation or component readiness.

This documents the accepted current source contract. It does not claim an
activated or qualified installed endpoint; the Toolkit/native journey remains
open.

## Request and result flow

All routes begin at `/console/forge/frontends`. They require the existing human
Console session. Mutations additionally require the same origin and JSON.
Host-health and native-console bearer tokens do not replace the human session.

| Route | Hook payload | Hook result |
| --- | --- | --- |
| `GET /console/forge/frontends/descriptor` | None; the hook is not invoked | The closed loaded declaration above |
| `GET /console/forge/frontends` | `{}` | A nonsecret inventory observation |
| `POST /console/forge/frontends/prepare` | `configuration` and `expected_revision` | An object containing `proposal` |
| `POST /console/forge/frontends/submit` | The exact `proposal` and human `approval` attribution | A nonsecret observation or explicit disposition result |
| `GET /console/forge/frontends/changes/<change_id>` | `change_id` | A nonsecret status observation |

The prepared proposal must contain `change_id`, `configuration` and
`expected_revision`. Its change ID uses 1-128 letters, numbers, dots,
underscores, colons or ASCII hyphens. The application may retain additional
proposal fields. The facade binds the complete proposal to the current host,
human account and expiry, then returns a protected proof alongside it. Proofs
expire after 300 seconds.

Successful responses include `protocol: 1` and the bound `host`. Prepare returns
`proposal` and `approval: {expires_at, proof}` at the top level. Inventory, submit
and status return the hook result under `observation`. Routes reject query
parameters, and GET requests must have no body. Requests, proposals, hook results
and the final response envelope use ordinary finite JSON, bounded to 256 KiB.

The human presenter submits the returned proposal and approval envelope with
`approved: true`. The facade checks the proof and supplies the hook with
`approval: {principal, expires_at}`. Browser-supplied principal, host, native
selector or permission fields cannot replace those trusted inputs.

`expected_revision` is application-owned JSON. The facade preserves it; the
application must enforce its own exact expected-revision comparison and
atomic commit. The HTTP facade is not a configuration database or lock owner.

## Negotiate an explicit submission disposition

A trusted application may return a typed result from its approved `submit`
hook using the original context:

```python
return self.context.frontend_submission_result(
    request, observation, known=outcome_is_known,
)
```

`outcome_is_known` must be an explicit Boolean chosen from the application's
knowledge of this configuration command's disposition. The helper binds the
original submitted change ID and complete current host. It does not interpret
an observation's status or establish success, native activation, readiness,
close, domain-job completion or delivery of the response. Direct integration
may use the shared `FrontendSubmissionResult` and
`FrontendSubmissionDisposition` types from `console_auth.py` instead.

A client opts in by sending exactly one
`X-Forge-Frontend-Submission-Outcome: 1` header on the authenticated
`POST /console/forge/frontends/submit`. A valid typed result then adds one field
to the existing successful protocol 1 envelope:

```json
{"submission_outcome": {"change_id": "change:one", "disposition": "known"}}
```

The nested object has exactly `change_id` and `disposition`. Disposition is
`known` or `unknown`; neither field may be null. The existing envelope's `host`
remains the complete current host, and `observation` remains opaque and unchanged.
Without the exact singleton offer, the response retains only `protocol`, `host`
and `observation`. Plain dictionary hook results do not add an outcome even
when their status says `succeeded`. Descriptors, Prepare and GET/status replies
retain their existing shapes.

A client may release uncertainty only for the currently entered submission when
it receives a valid negotiated `known` outcome matching that exact change ID
and current host. Keep submitted IDs, spent calls and old approvals spent. An
unknown, absent, old, malformed, mismatched or lost outcome stays conservative;
an older server may ignore the offer. The field supplies no retry permission,
activation proof or restored approval.

## Retain changes and recover

Use the application's existing durable state and `start()`/`stop()` hooks.
The owner must retain a stable change ID, handle duplicate submissions, enforce
revision checks and reconstruct its committed configuration after restart.
Configured, activated, native input completed and transport reply delivered are
separate observations. Return only observations the component actually knows.

After a submission reaches the owner, a caught owner exception or invalid result
returns HTTP 503 with `status: unknown` and the exact change ID. A side effect may
already have occurred. Refusals before admission use their own error response;
a lost response or cancellation may provide no HTTP 503 at all. Read the exact
change's status through the same owner before taking another action. The facade
does not retry, roll back or infer success from a lost response. Responses from
the frontend handler disable caching. An undeclared owner returns HTTP 501 and
is never forwarded to native RPC.

## Keep credentials and package updates with their owners

Configuration may reference a declared private field under its matching key:
`{"DECLARED_FIELD_NAME":{"private_field":"DECLARED_FIELD_NAME"}}`.
Literal credentials stay in the
existing private settings flow. Do not return passwords, cookies or connector
tokens in proposal or observation JSON. The facade refuses literal values for
declared private fields by recursively checking their keys. This is not general
secret detection; the component must keep other confidential values out of its
results as well.

The frontend hook manages components of the currently running application.
Changing private field declarations, installing dependencies, replacing the
application package or starting a stopped installation belongs to Forge's
existing deployment/update transaction. This hook creates no second supervisor.

Source checkouts include the maintained boundary cases in
`installer/tests/test_frontend_management.py` and
`installer/tests/test_frontend_submission_disposition.py`. They cover
source binding, human session separation, exact proposal approval, private
field references and uncertain submission readback. They do not demonstrate a
working third-party component or the pending native Toolkit bootstrap join.

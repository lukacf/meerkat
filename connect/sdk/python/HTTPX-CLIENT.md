# Preparing a borrowed HTTPX client

The host prepares each HTTPX client once, before sharing it with Toolkit
Gmail, Google Calendar, Home Assistant, Slack or Drive operations:

```python
import httpx
from meerkat_toolkit.httpx_client import prepare_httpx_client

async with prepare_httpx_client(httpx.AsyncClient(
    base_url=selected_api_origin,
    auth=host_selected_auth,
    event_hooks=host_selected_hooks,
    trust_env=False,
)) as client:
    # Construct operations using this already prepared client.
    await run_authorized_operations(client)
```

`prepare_httpx_client` returns the same client. It adds one first response hook
and preserves the relative order of every existing hook. Calling it again is
idempotent. Preparation changes no account, credential, cookie, request header,
endpoint, proxy, transport or client lifetime. The host still owns configuration
and eventual client closure. The example's network settings are host choices.

This explicit setup is required even when there are no application response
hooks. HTTPX authentication flows can also consume a response before `send`
returns. The first hook installs retained response-stream cleanup without
suspending, before application hooks and authentication response handling.
Ordinary response hooks, including hooks that read a response body, remain
supported. Operations retain their one send and wait for its cleanup when
cancellation or a deadline occurs. They never detach cleanup or retry a request.
The deadline starts cancellation; it cannot guarantee when arbitrary host
transport cleanup finishes.

Operations check that the preparation hook remains first before starting work
and again before sending. The shared validator raises `HttpxClientNotPrepared`,
a `ValueError` with `request_attempted=False`, when the hook is missing or
reordered. Some connectors preserve a service-specific error envelope: Drive
projects this refusal to `DriveError` with kind `CLIENT` and write outcome
`NOT_ATTEMPTED` (or `NOT_APPLICABLE` for a read). These are local refusals before
HTTPX authentication or provider entry. Hosts that access credentials or files
before invoking operations should call `require_prepared_httpx_client(client)`
first. That function only validates; it never changes a shared client.

Prepare before concurrent use. While operations borrow the client, the host
must keep hook ordering and response-stream ownership stable. Concurrent host
reconfiguration is outside this contract; a mutable public HTTPX configuration
cannot be made immutable by a per-call check. Auth, hooks and transports must
also preserve the selected request and must not retry, retarget or suppress
cancellation. A transport owns resources until it returns a response. Toolkit
owns the retained response cleanup after that handoff, and never closes the
borrowed client itself.

This is an explicit interface correction in the unpublished SDK. Previous
examples that handed an arbitrary client directly to operations need the
preparation step at their host construction site. Tests of the older interface
and its cleanup failures remain recorded separately from prepared-client tests.

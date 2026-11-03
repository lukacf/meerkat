# Home Assistant operations

Before sharing a client, the host must call `prepare_httpx_client(client)` once.
See [HTTPX client preparation](HTTPX-CLIENT.md) for the setup and ownership contract.
All client arguments in the examples below are already prepared.

Install the release wheel with the Home Assistant extra in your Python environment:

```sh
python -m pip install 'meerkat-toolkit[home-assistant] @ https://github.com/lukacf/meerkat-connect/releases/download/v0.9.0/meerkat_toolkit-0.9.0-py3-none-any.whl'
```
Import `HomeAssistantOperations` from `meerkat_toolkit.connectors.home_assistant`.
The package does not require MobKit for these operations.

The application supplies an authenticated `httpx.AsyncClient` with one selected
Home Assistant base URL. The application continues to own that client, its
credential, network/TLS configuration, authorization, rotation and closure.
The operation library does not look up environment variables or credentials,
prompt a model to confirm authorization, install servers or run background tasks.

```python
from meerkat_toolkit.connectors.home_assistant import HomeAssistantOperations

async def read_selected_light(authenticated_client):
    operations = HomeAssistantOperations(authenticated_client)
    return await operations.get_state("light.kitchen")

async def invoke_authorized_service(authenticated_client, selected_data):
    # The host has already authorized this exact instance, service and data.
    operations = HomeAssistantOperations(authenticated_client)
    return await operations.call_service("light", "turn_on", data=selected_data)
```

`list_states()` returns the complete selected instance states response without
client-side entity filtering. `get_state(entity_id)` preserves the complete entity object, including nested
attributes and context. `list_services()` preserves the advertised service
metadata instead of reducing it to display names. `call_service(domain, service,
data=..., return_response=False)` sends one POST. A successful result contains
`changed_states`, `response_requested` and `service_response`. The optional
`return_response=True` profile is the provider's explicit response mode; the
caller must select it for a service that requires a response. Changed states can
be empty and do not establish a device's final physical state. The provider's
[REST contract](https://developers.home-assistant.io/docs/api/rest/) defines
these two response shapes.

This is an explicit REST selection. Home Assistant's Assist MCP endpoint has its
own exposed-entity surface. Do not use REST as an automatic fallback when an MCP
tool or an entity is unavailable or denied. The host must authorize the REST
credential and the exact operation separately; this module supplies no requester
identity, C1 enforcement or cross-user policy. See the provider's
[MCP integration](https://www.home-assistant.io/integrations/mcp_server/).

## Target and transport profile

- The client base URL (or explicit `base_url=`) is captured at construction,
  including an encoded reverse-proxy prefix. An explicit `auth=` is used only for
  this operation object; the borrowed client is not mutated. Defaults continue
  to use the host-authenticated client. Userinfo, a query and a fragment in that base URL are refused. Later
  changes to `client.base_url` do not retarget this instance.
- Domain, service and the two entity-ID parts accept lowercase ASCII letters,
  digits and underscore. There is no normalization, fuzzy selection or arbitrary
  path/URL input. Other naming profiles fail locally before transport entry.
- Client default query parameters are excluded. `return_response` is emitted
  only when explicitly requested. Caller service data is not supplemented with
  a target or filtered by a second entity registry.
- Redirects are refused even when the borrowed client's default follows them.
  The library sends `Accept-Encoding: identity` and accepts only identity-encoded
  `application/json` metadata responses or the image media profile below.
  Duplicate object keys, non-finite numbers,
  malformed JSON and unexpected result shapes are errors.
- The retained response body is limited to 1 MiB by default. The default one
  operation deadline is 20 seconds across send and streamed reading. Both are
  configurable. Expiry starts response closure, which is still awaited; this is
  not a promise that arbitrary host transport cleanup finishes by that deadline.
- Standard HTTPX timeout/transport failures are projected into safe error kinds.
  The host must not configure an auth flow, hook, proxy or custom transport that
  retargets or replays service calls. These client internals are host-owned and
  cannot be verified by this library. The library contains no request retry.
  Account isolation requires separate correctly selected clients; sharing or
  mutating their authentication remains the host's responsibility.

## Failures and uncertain writes

`HomeAssistantError` contains `kind`, `status_code`, `write_outcome` and an optional
decimal `retry_after_seconds`. Provider response bodies, URLs, credentials and
the underlying HTTP exception are not included in its diagnostic text or chained
exception. HTTP-date Retry-After is not interpreted in this initial profile.
Returned operation data remains provider data and may be private. Host logging,
hooks and traces must apply their own data rules.

For reads, `write_outcome` is `NOT_APPLICABLE`. Local identifier/JSON/option
validation raises `ValueError` before a request is sent. Once a service attempt
starts, every failure has `WriteOutcome.UNKNOWN`: HTTP error status, malformed
success receipt, timeout, oversize body, redirect or transport loss. A 400 or 401
does not prove that a handler never ran. Call cancellation raises
`ServiceCallCancelled`, an `asyncio.CancelledError` subclass whose
`write_outcome` is also `UNKNOWN`. Do not convert it into a successful result.

This conservatism follows the actual provider owner. The pinned
[service view](https://github.com/home-assistant/core/blob/6cd7e61bad577dd450e26dca2680fd80770543b1/homeassistant/components/api/__init__.py#L383-L477)
shields entered service execution from connection cancellation. Its
[provider tests](https://github.com/home-assistant/core/blob/6cd7e61bad577dd450e26dca2680fd80770543b1/tests/components/api/test_init.py#L545-L580)
exercise completion after client disconnect. A host can inspect current state or
use service-specific reconciliation, but this library neither retries nor claims
that disconnect stopped remote work.

## Evidence and limits

The loopback suite uses real HTTPX connections and retained local HTTP handlers.
It covers nested result fidelity, both service profiles, separate account
selection, redirect refusal, local refusal, provider errors, bounded bodies,
malformed data, timeout, cancellation and connection loss. Its handlers are
fixtures, not a Home Assistant installation. Live account support, interactive
authorization, device effects, packaging/release validation, host migration,
TypeScript operations and the rest of Toolkit M4/M8 remain separate work.

The implementation is original Toolkit code based on the public protocol, not
a copy of any host's environment/global-client wrappers. Home Assistant core
`6cd7e61bad577dd450e26dca2680fd80770543b1` is the pinned source oracle and is
[Apache-2.0 licensed](https://github.com/home-assistant/core/blob/6cd7e61bad577dd450e26dca2680fd80770543b1/LICENSE.md).
HTTPX is an optional runtime dependency. The repository's Toolkit license
declaration is `MIT OR Apache-2.0`; the Python package's existing license metadata
and release distribution still need the normal packaging review.

## State collection and camera stills

`get_camera_image("camera.front")` returns immutable `CameraImage(data,
content_type)`. Bytes are preserved exactly. `content_type` is the normalized
media-type portion of the provider's Content-Type header, without parameters.
The operation accepts one successful HTTP 200 response with a nonempty `image/*`
body. It does not assume JPEG, decode the image or establish what the image shows.
Empty, non-image, malformed-length and truncated bodies are explicit read errors.
A non-image login page is not a camera still. Full-body requests exclude inherited
range, conditional and method-override headers, as well as default query values.

The JSON limit does not cap media. `max_media_bytes=None` is the camera default,
matching earlier buffered-still behavior without a new size cap.
A host may supply a positive media byte limit. Images are buffered in memory, so
hosts should select an appropriate limit for their deployment. Neither JSON nor
media limits silently truncate a successful result. Timeout and cancellation join
response cleanup, including EOF auto-close and prepared response hooks. Borrowed
clients remain open for their owner to close.

The pinned Home Assistant [states owner](https://github.com/home-assistant/core/blob/6cd7e61bad577dd450e26dca2680fd80770543b1/homeassistant/components/api/__init__.py#L208-L237)
filters states using its authenticated user's entity permissions. Toolkit does
not repeat or replace that policy. The pinned [camera view](https://github.com/home-assistant/core/blob/6cd7e61bad577dd450e26dca2680fd80770543b1/homeassistant/components/camera/__init__.py#L782-L838)
checks authentication and availability, then returns image bytes with the camera's
content type. These are protocol observations, not proof of any live account,
entity grant, image decoder compatibility or physical effect.

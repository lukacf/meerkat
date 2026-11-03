# Borrow a host-authenticated Slack, Gmail, Calendar or Drive client

An embedding host can expose the existing selected service tools using its own
prepared HTTPX client. The host keeps credentials, authentication, account
selection and the client lifetime. No credential is read from the environment
by this entry point, and no new HTTP client or authentication object is created.

```python
from meerkat_toolkit.adapters.services_mcp import (
    create_profile_from_client,
    create_server,
)
from meerkat_toolkit.httpx_client import prepare_httpx_client

# configuration is the existing validated service Configuration.
# host_client is an AsyncClient bound by the host to the selected account.
# Prepare before sharing it with profiles or starting any calls.
prepare_httpx_client(host_client)
profile = create_profile_from_client(configuration, host_client)
try:
    server = create_server(profile)
    # Run server with the embedding host's existing MCP transport/lifecycle.
finally:
    await profile.aclose()

# The host closes host_client only after all its borrowers have joined.
```

The client base URL must equal `https://slack.com/` for Slack,
`https://gmail.googleapis.com/` for Gmail, or `https://www.googleapis.com/` for
Google Calendar and Drive, as compared by HTTPX's normalized URL
type. Custom origins, path prefixes, userinfo, queries and fragments are refused.
A closed, unprepared or non-HTTPX client is refused with the fixed
`ConfigurationError` message. The function checks preparation without modifying
the client. It does not probe the provider or verify credentials.

The existing configuration remains the source of the operation subset, response
and message bounds, Gmail `user_id`, and Slack's optional `allowed_channel_ids`
ceiling. The latter accepts a JSON array of at most 256 unique exact channel
IDs: `[]` denies channel calls and omission adds no channel limit. Broad
conversation listing cannot be selected with a ceiling. The validated array
is frozen and applies equally to borrowed and owned clients, including resumed
reads. It is a host-configured routing limit, not a grant or message-content
filter. Gmail's optional `allowed_recipient_addresses` similarly limits every
To/Cc/Bcc mailbox in `send_message`. Omission adds no limit; `[]` denies every
send. Restricted configurations refuse opaque `send_raw` and `create_draft_raw`.
Recipient comparison preserves local-part case and normalizes only ASCII domain
case; aliases and internationalized-domain equivalents are not inferred. A
denied recipient returns `permission` and `not_attempted` before connector,
call-time authentication or transport entry. The raw Python/TypeScript APIs do
not enforce this configured-services limit. The host still owns current grants,
sender and content approval.

The `google-calendar` profile fixes `calendar_id` in the same configuration.
Only explicitly selected operations are exposed: `list_events_page`, `get_event`,
`insert_event`, `patch_event` and `delete_event`. Read-only selections cannot write;
tool arguments cannot select another calendar. A page preserves all provider
fields and tokens. A `nextPageToken` means more results remain for that query,
even if its `items` is empty. The caller must choose each subsequent read;
there is no traversal, synchronization or complete-agenda guarantee.
Writes use the existing 2 MiB request-body bound. An exact quoted `expected_etag`
conditions patch/delete; omission or null explicitly requests an unconditional
write. Failures after connector entry remain unknown and never trigger retries.
The host owns calendar/account correspondence, event/attendee authorization and
reconciliation. Notification options are not exposed, and provider defaults may
send email. A successful event receipt does not prove notification delivery.
The host-only Google reconnect/refresh helpers support explicit `service="calendar"`
or `service="drive"` with their documented scopes; omission retains Gmail.
They do not own login presentation or credential persistence. This profile
borrows the authenticated client supplied by the host's existing owner.

The `google-drive` profile selects list/get metadata and bounded content reads.
Its `max_content_bytes` applies to complete downloaded/exported bytes. There is
no tool-selected output path or account switch and no per-file routing ceiling.
A fixed metadata mask includes continuation, source link and download/shortcut
fields. The caller chooses each page and each content format explicitly. MCP
returns exact text where suitable or a base64 embedded resource, not extracted
text or a model-readability guarantee. The same limits apply to owned and
borrowed clients; original Drive disk download methods are not exposed.

Its `token_env` is still a required,
validated reference because this is the same installed profile configuration.
In this host-client entry point that reference is unused, including when the
named variable is missing. Keep the real configured reference; do not invent a
placeholder or put a credential value there. The CLI and `create_profile()`
continue to read the selected environment variable once and own the HTTP client
they create. The CLI and MCP tool arguments do not expose the borrowed-client API.

## Authentication and request ownership

The host client supplies its normal headers and HTTPX authentication flow. An
existing host credential owner can supply a current credential before each
explicit request, including rotation for the same selected account. Toolkit
does not implement token storage, OAuth consent, refresh or auth retries.

Host auth flows, event hooks, transports and proxies must preserve the selected
origin, account, method and content. They must not replay a request, retarget it
or suppress cancellation. In particular, a host auth flow must not resend a
Gmail write after a 401 or an uncertain response. These are obligations of the
trusted host composition, not protections inferred from a client flag. Toolkit
uses its existing connectors, disables redirect following for their requests,
and returns existing provider failures and write uncertainty without retries.

The current Python source also projects Gmail's optional allowlisted
`provider_reason` through MCP errors. For HTTP 403, documented rate reasons use
`rate_limited`, daily quota exhaustion uses `quota_exceeded`, and domain policy
uses `permission`; ambiguous or unavailable diagnostics keep generic
`permission` with no reason. The diagnostic read uses at most the smaller of
the configured response bound and 65,536 bytes under the original deadline and
cleanup owner. Entered writes still report an unknown
outcome, and the host owns every decision about retry, reconnect or consent.
Previously built services wheels do not acquire this change automatically.

Calendar and Drive also classify HTTP 403 as `rate_limited` when a bounded JSON
diagnostic consistently identifies `usageLimits/rateLimitExceeded` or
`usageLimits/userRateLimitExceeded`. Other, mixed or unusable evidence stays
`permission`. The read shares the original deadline and cleanup owner, has the
same 65,536-byte cap, and never exposes provider prose or retries a request.
Entered writes remain `unknown`. See Google's [Calendar error guide](https://developers.google.com/workspace/calendar/api/guides/errors)
and [Drive error guide](https://developers.google.com/workspace/drive/api/guides/handle-errors).

The host owns TLS and proxy settings and must keep the prepared response hook
first while the client is borrowed. Preparation preserves the remaining host
hooks and protects response cleanup; see
[`httpx_client.py`](src/meerkat_toolkit/httpx_client.py). A host must not change
the account behind a profile, close the client during a call, or replace its
cleanup ownership. Separate selected accounts need separate host-bound clients
and profiles. Gmail `user_id` is routing, not proof that the credential represents
that mailbox. Provider account observations, granted scopes, approved writes and
native attachment remain separate host decisions.

## Closing a profile

`await profile.aclose()` closes admission and waits for an active profile call
and its response cleanup. Repeated closure and cancellation of the close waiter
retain that cleanup. Subsequent tool calls return the existing `closed` failure.
Closing a borrowed profile never closes its client. Another profile borrowing
the same client for the same account remains usable; the host must join every
borrower before closing the shared client.

The profile's lock serializes its own calls, not calls from other borrowers.
The host is responsible for any shared authentication concurrency requirements.
Tools continue to expose only the configured operation schemas, with no URL,
credential, client or account-switch parameter. Profile representation omits
the host client and operation objects; this does not make arbitrary host logs
or authentication exceptions safe to publish.

The source tests exercise real in-process MCP clients with mock HTTP providers,
credential rotation between explicit calls, failure without replay, account and
operation isolation, and joined cancellation. They do not establish live Slack
or Google authorization, a native credential binding or a deployed integration.

# Google Calendar Events operations

Before sharing a client, the host must call `prepare_httpx_client(client)` once.
See [HTTPX client preparation](HTTPX-CLIENT.md) for the setup and ownership contract.
All client arguments in the examples below are already prepared.

Install the optional `meerkat-toolkit[google-calendar]` profile. Import
`GoogleCalendarOperations` from `meerkat_toolkit.connectors.google_calendar`.
The host supplies an authenticated HTTPX client and selects one calendar. The
library does not acquire credentials, select an account, infer intent, authorize
a write, expose model tools, or traverse pages.

```python
operations = GoogleCalendarOperations(
    host_authenticated_client,
    calendar_id=selected_calendar_id,
    base_url="https://www.googleapis.com/",
)
page = await operations.list_events_page(
    time_min="2026-10-01T00:00:00+02:00",
    time_max="2026-10-08T00:00:00+02:00",
    single_events=True,
    order_by="startTime",
    max_results=250,
)
# The host decides whether and when to request page["nextPageToken"].
current = await operations.get_event(selected_event_id)
updated = await operations.patch_event(
    selected_event_id,
    {"location": None},
    expected_etag=current["etag"],
)
```

The five operations are `list_events_page`, `get_event`, `insert_event`,
`patch_event`, and `delete_event`. Listing supports explicit time bounds, search,
page token, page size, expansion, ordering and deleted-event inclusion. Other
Calendar APIs, synchronization, notification options, attachments, conference
creation parameters and partial-response `fields` are outside this profile.
Unsupported arguments refuse locally. No defaults for time, attendees,
notifications or event content are manufactured.

The host owns the borrowed client's authentication, refresh, TLS, proxy choice,
credential/account correspondence and eventual closure. An optional per-request
HTTPX `auth` object supports host account binding without mutating shared client
headers. The endpoint is captured at construction. Host auth, hooks and
transports must not retarget, replay, suppress cancellation or change the selected
credential during an operation. These are trusted host components, not a sandbox.

Identifiers are encoded as single path segments. Bodies preserve omission,
explicit null, empty arrays/strings and false. Expected ETags must be one exact
quoted provider value, not a wildcard or list. An absent expected ETag means an
explicit unconditional patch/delete; the library never substitutes one for a
missing conditional value. Inherited query defaults and conditional/method
override headers cannot change the operation. Redirects are not followed.

An event response preserves all provider fields. Normal events and write
receipts require a nonempty matching ID, quoted ETag and `updated` version.
Cancelled reads may contain the provider's minimal tombstone. Insert/patch use
HTTP 200 event receipts; delete uses HTTP 204 with an empty body. Reading one page
never proves that the entire calendar was read. See Google's [Events reference](https://developers.google.com/workspace/calendar/api/v3/reference/events),
[list contract](https://developers.google.com/workspace/calendar/api/v3/reference/events/list)
and [delete contract](https://developers.google.com/workspace/calendar/api/v3/reference/events/delete).

Response bodies have a configurable byte bound and deadline. JSON rejects
duplicate members and nonfinite numbers, including exponent overflow. Error
objects contain bounded categories, HTTP status and numeric Retry-After only;
provider text, event contents, URLs, credentials and arbitrary host exceptions
are not retained. Every error from an attempted write is conservatively
`WriteOutcome.UNKNOWN`, including provider refusals. HTTP 412 is a typed
precondition failure, never an instruction to fetch a newer version and overwrite
it. Google's [version contract](https://developers.google.com/workspace/calendar/api/guides/version-resources)
describes conditional writes; this library adds no retry policy.

Cancellation retains and joins entered transport cleanup through repeated caller
cancellation. A write raises `CalendarWriteCancelled`, carrying unknown outcome.
A timeout or lost response can follow a committed provider write. No automatic
retry, new event ID, fallback backend, rollback or exactly-once guarantee follows.
The host must reconcile against its selected identity and stable operation key.

## Configured MCP operations

The standalone `meerkat-toolkit-services` component exposes all five operations
through an explicit nonempty operation subset and a fixed host-selected
`calendar_id`. Its default read example does not enable writes. Event objects
and conditional ETags use the same connector contract above; the adapter does
not add retries, calendar selection or a new credential owner. See the
[configured service guide](../services/README.md#calendar-writes) for operation
arguments, scope requirements and notification limitations.

## Host adoption

A host's command runner can dispatch Calendar events list/get/insert/patch/delete
and conditional patch/delete through these operations. Unsupported Calendar
commands or output/upload/partial-response options should refuse rather than
fall back to a subprocess. The host's Google token broker supplies an ephemeral
request binding once; Calendar operations do not use an authentication-substring
retry loop. A typed HTTP 401 invalidates only that account's broker cache entry,
then raises the original failure. Refresh can occur on a later explicit call;
this attempt never refreshes or repeats the provider operation.

The host keeps account normalization, raw-write denial for internal calendars,
source-gap policy, origin stamping, its write-version ledger, semantic
publication, ETag revalidation, deterministic IDs and final readback. A host can
add a typed `write_outcome: "unknown"` to mutation failure receipts when the
actual Calendar runner error carries it. Read failures and untyped prose do not
create that field. A later explicit stable-key publication may reconcile a
previously committed write; the failed invocation itself cannot report
publication success.

Full Calendar API coverage, production OAuth/account readiness, artifact
installation and a complete host migration are separate acceptance work. Local
loopback and source-composition tests do not establish live Google readiness or
a deployed system.

## Provenance

This module is original implementation informed by official Google REST contracts
and existing Toolkit HTTP/cancellation patterns. No Google client-library source
or host implementation body is included in the module. Host-specific binding and
policy code stays in the host. The SDK artifact's existing mixed-source license
declaration still applies to the package as a whole; this guide makes no new
grant for its other private-source modules.

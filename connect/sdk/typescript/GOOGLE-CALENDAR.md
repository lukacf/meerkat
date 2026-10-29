# Google Calendar Events operations

Import `GoogleCalendarOperations` from `@rkat/toolkit/connectors/google-calendar`.
This Node 22+ ESM connector adds no dependency. It uses the existing host-selected
authenticated Fetch convention and shared strict JSON utility.

```ts
import { GoogleCalendarOperations } from "@rkat/toolkit/connectors/google-calendar";
import type { GoogleCalendarContext } from "@rkat/toolkit/connectors/google-calendar";

// The host connection owner selects the endpoint, calendar and credential closure.
declare const selectedCalendar: GoogleCalendarContext;
const calendar = new GoogleCalendarOperations(selectedCalendar);
const page = await calendar.listEventsPage({
  timeMin: "2026-10-01T00:00:00+02:00",
  timeMax: "2026-10-08T00:00:00+02:00",
  singleEvents: true,
  orderBy: "startTime",
  maxResults: 250,
});
const current = await calendar.getEvent("selected-event-id");
// Host admission and permission cover this exact change before invocation.
if (typeof current.etag === "string") {
  await calendar.patchEvent(current.id, { location: null }, { expectedEtag: current.etag });
}
```

## Selection and host ownership

`GoogleCalendarContext` contains `baseUrl`, `calendarId` and
`fetch: (request: Request) => Promise<Response>`. The normal base is
`https://www.googleapis.com/`. Use a canonical absolute HTTP(S) URL without
userinfo, query, fragment or hidden normalization. An explicit proxy prefix is
retained; a trailing slash is added to a nonempty prefix. `calendarId` is the
host's exact selection, including Google's `primary` selector when intended.
Calendar and event selectors are encoded as single segments; dot-only selectors
are refused. Text selectors and query values must be nonempty valid Unicode,
without controls and within 8 KiB UTF-8. No normalization or trimming occurs.

Construction captures the endpoint, calendar and Fetch function. Later changes
to context properties cannot retarget the instance. The closure binds the actual
credential; the calendar string is not an authenticated account or an operation
grant. Separate selected accounts use separate closures even at the same URL.
The host retains OAuth, credential refresh, account correspondence, admission,
resource permission, TLS, DNS/proxy/egress, transport pools and retirement.

The borrowed Fetch must preserve method, destination, body, headers and signal.
It must not retry, retarget, add default parameters, switch credentials or replay
a write during refresh. Requests specify manual redirects, omitted cookies and
no caching. The library rejects redirected responses, but cannot undo effects of
a nonconforming host closure. This is a trusted in-process binding, not a sandbox
against hostile JavaScript or Proxies. It never closes the host transport or reads
ambient credentials. REST is explicitly selected, with no MCP or CLI fallback.

## Operations and representations

- `listEventsPage({timeMin, timeMax, q, pageToken, maxResults, singleEvents,
  orderBy, showDeleted, signal}?)` returns one complete page. All query options
  are optional and omitted when unspecified. Page size is 1-2500. `orderBy` is
  `updated`, or `startTime` with `singleEvents: true`. Flags must be booleans.
  Time/query strings remain exact; the provider validates their domain syntax.
- `getEvent(eventId, {signal}?)` returns the selected event, including a cancelled
  tombstone. `insertEvent(event, {signal}?)` sends the supplied object once.
- `patchEvent(eventId, patch, {expectedEtag, signal}?)` and
  `deleteEvent(eventId, {expectedEtag, signal}?)` use the exact `If-Match` when
  supplied. The supported ETag profile is one quoted ASCII opaque value, with no
  wildcard, weak prefix, whitespace, control or list. Omission means an explicit
  unconditional operation; null or malformed ETags refuse before Fetch.

Objects retain every provider field, including event times, recurrence, attendee
data, empty arrays/strings, null, false and unknown fields. A list requires
`kind: "calendar#events"`; its absent items remain absent. Nonempty continuation
and synchronization tokens remain opaque. An empty page can still have another
page, and this library never follows one. See Google's
[list contract](https://developers.google.com/workspace/calendar/api/v3/reference/events/list).

Normal event receipts require a nonempty ID, quoted ETag and updated field.
Cancelled reads may contain the minimal tombstone; writes require the complete
receipt identity/version fields even if status is cancelled. Get/patch receipts
must match the selected ID, and insert matches a caller-supplied body ID when
present. The caller chooses valid provider IDs; this library does not create or
repair them. See [event representations](https://developers.google.com/workspace/calendar/api/v3/reference/events),
[get](https://developers.google.com/workspace/calendar/api/v3/reference/events/get)
and [insert](https://developers.google.com/workspace/calendar/api/v3/reference/events/insert).

Event bodies are frozen before the first suspension using the existing plain JSON
snapshot utility. Patch omission remains omission; arrays supplied to a patch
replace the provider's arrays rather than merge them. A supplied patch ID must
match its selected event. This module does not manufacture attendees, times,
notifications or defaults. See Google's
[patch contract](https://developers.google.com/workspace/calendar/api/v3/reference/events/patch).
It never substitutes get-plus-update for a patch.

Insert/patch/get/list require HTTP 200 and an appropriate JSON receipt. Delete
requires HTTP 204 with an empty body and returns `undefined`, not an event.
Other success codes are refused. See Google's
[delete contract](https://developers.google.com/workspace/calendar/api/v3/reference/events/delete).
`CalendarError` with kind `precondition` represents HTTP 412. A stale conditional
write does not fetch a fresh ETag or retry. See
[conditional modifications](https://developers.google.com/workspace/calendar/api/guides/version-resources).
These primary contracts were checked on 2026-10-01.

Runtime option objects reject unknown fields, accessors and non-plain objects,
not only TypeScript type errors. Synchronization, `fields`, notification options,
conference/attachment feature parameters, other Calendar APIs and full provider
schema validation are outside this profile. A provider may ignore unsupported
feature body fields according to its API defaults; passing such fields does not
establish feature support. Select a suitable explicit implementation when those
features are required. Reading a page does not prove complete calendar coverage.

## Bounds, cancellation and uncertainty

Constructor options default to `timeoutMs: 20_000`, `maxResponseBytes: 2_097_152`
and `maxRequestBytes: 2_097_152`. One deadline covers Fetch, streamed reading and
receipt validation; progress does not reset it. Size bounds limit data consumed
by this library, not allocations/buffering inside the host transport or the
initial plain-data serialization. Request options and JSON bodies are captured
before transport suspension.

Responses require identity encoding and UTF-8 `application/json` (except empty
deletion receipts). The unchanged strict parser rejects duplicate keys, malformed
syntax/UTF-8, comments, trailing commas, nonfinite numbers, unsafe integers and
more than 128 container levels. Supported fractional numbers use binary64.
Requests reject undefined/functions/symbols, accessors, sparse/extended arrays,
non-plain objects, cycles, unsafe/nonfinite numbers and negative zero. Getters
and toJSON hooks are not invoked. This profile differs from Python's arbitrary
integer support. Invalid local inputs produce safe TypeErrors before Fetch.

Abort/deadline initiates transport and reader cancellation. The entered Fetch and
response cleanup remain retained and joined before the call settles, including
a late response from a host that ignores abort. Cleanup may take longer than the
deadline; an uncooperative host can keep the call pending. There is no detached
cleanup or hard cleanup bound. Local settlement does not stop or undo a remote
effect. `CalendarCancelled` is a distinct typed rejection; JavaScript does not
have Python's cancellation exception hierarchy.

`CalendarError` exposes fixed `kind`, `writeOutcome`, optional `statusCode` and a
bounded decimal `retryAfterSeconds`, without raw bodies, event contents, URLs,
credential data, arbitrary host exceptions, causes or caller abort reasons.
Returned provider objects can themselves be private. Reads have outcome
`not_applicable`. Every failed attempted write is conservatively `unknown`, even
HTTP refusals, redirect, malformed receipt, timeout, loss and cancellation. An
already-aborted write sends nothing but uses the same conservative cancellation
outcome. Retry-After is information, never replay authorization.

HTTP 403 is `rate_limited` only when bounded JSON consistently identifies the
documented `usageLimits/rateLimitExceeded` or `usageLimits/userRateLimitExceeded`
reason. Unknown, mixed or unusable evidence stays `permission`. Diagnostic reads
use at most the smaller of `maxResponseBytes` and 65,536 bytes under the same
deadline and cleanup owner. Provider prose is not exposed. See Google's
[error guide](https://developers.google.com/workspace/calendar/api/guides/errors).

The host must reconcile uncertainty using its selected calendar/account and
stable operation identity. An empty search or a new consent does not establish
that an earlier write failed. There is no automatic retry, fresh event ID,
rollback, execution journal or exactly-once guarantee.

## Provenance and verification

Original Toolkit implementation under MIT OR Apache-2.0, informed by the linked
Google documentation, existing Toolkit Python Calendar behavior and Toolkit
TypeScript Fetch/JSON conventions. No Google SDK or host implementation body
is included. The package retains its existing license texts and notices.

`tests/google-calendar.test.mjs` imports the public package surface and exercises
real loopback HTTP plus controlled host cleanup. Its cancellation assertions
observe local response cleanup separately from remote handler completion.
Source and installed-artifact checks remain distinct. These fixtures establish
neither live Google/OAuth readiness nor native admission, grant association,
catalog activation, full M4/M8 coverage or a host's TypeScript adoption. No test
result is implied by this guide or these commands:

```sh
npm run typecheck
npm run build
node --test tests/google-calendar.test.mjs
```

# Google Calendar 0.2.0 support record

This additive version preserves `meerkat-toolkit/google-calendar-api-setup` and
its single `google-calendar-api-setup.service` service-connector export. Version
`0.1.0` remains byte-for-byte unchanged and guidance-only. The agent chooses the
exact version, library digest and strategy. There is no ranking resolver,
automatic upgrade or fallback between the two choices.

| Explicit strategy | Available material | Boundary |
| --- | --- | --- |
| `hosted-mcp-preview` | Google's documented Streamable HTTP endpoint and typed setup prerequisites | Restricted Developer Preview; no account, scope, current-schema or role-readiness receipt |
| `toolkit-rest` | Python `GoogleCalendarOperations` and a frozen source-composition checkpoint | Guidance-only activation plan; existing host owners supply authentication, policy and service-export binding |

## Hosted MCP developer preview

The [official Calendar MCP guide](https://developers.google.com/workspace/calendar/api/guides/configure-mcp-server)
documents `https://calendarmcp.googleapis.com/mcp/v1`, Streamable HTTP and OAuth
2.0. The selected account and Cloud project must be enrolled in the
[Workspace Developer Preview Program](https://developers.google.com/workspace/preview).
Enable both `calendar-json.googleapis.com` and `calendarmcp.googleapis.com` with
the necessary project permissions. Configure the actual host's consent/audience,
OAuth client and redirect URI; a provider example redirect is not registration
for this host.

Preview features are restricted to permitted preview use. They cannot be used in
public applications before general availability; access outside the registered
domain/company requires any explicit permission offered and granted by Google.
An entry in this catalog does not establish enrollment, permission or public
availability. Recheck current terms and eligibility during setup.

The guide advertises event creation, retrieval, listing, updates, deletion,
responses and scheduling suggestions. However, its sample OAuth scopes are:

- `https://www.googleapis.com/auth/calendar.calendarlist.readonly`
- `https://www.googleapis.com/auth/calendar.events.freebusy`
- `https://www.googleapis.com/auth/calendar.events.readonly`

Those scopes do not authorize mutations. The recipe is marked `read_only: false`
because the provider advertises mutating tools, not because an account has consented
to them. The existing credential and policy owners must verify the selected
account/calendar, current tool schema and [operation-specific scope](https://developers.google.com/workspace/calendar/api/auth).
No scope expansion, mutation grant or consent follows from manifest validation.

Google's [MCP security guidance](https://developers.google.com/workspace/guides/configure-mcp-security)
requires screening prompts and responses. The host chooses Model Armor or its
documented alternative accepted by users, including processor, permissions,
routing and logging. Model Armor logging can include entire payloads. This recipe
does not enable an external processor or authorize sending calendar content to it.

The portable endpoint has no inline headers, credentials or query secrets. An
offline-valid `ActivationManifest` remains a declaration. Existing owners must
provide authenticated account, scope, schema, policy and role-readiness evidence
before reporting usable tools. Refusal or preview unavailability stays a refusal;
it never selects REST automatically.

## Toolkit Python REST operations

Import `GoogleCalendarOperations` from
`meerkat_toolkit.connectors.google_calendar`. The
[Python guide](../../../../../../sdk/python/GOOGLE-CALENDAR.md) documents the
`google-calendar` optional profile: Python 3.11+, HTTPX `>=0.28.1,<0.29` and AnyIO
`>=4.13,<5`. The host provides the authenticated HTTPX client, selected API base
and calendar, with optional per-request auth binding. The library owns no
credential store, OAuth flow, refresh policy or second runtime.

Coverage is `list_events_page`, `get_event`, `insert_event`, `patch_event` and
`delete_event`. The caller owns traversal of opaque page tokens. Unsupported
Calendar options, including partial-response `fields`, refuse locally. Normal
event/write responses retain exact event ID, ETag and returned `updated` version;
cancelled reads may carry minimal provider tombstones. Conditional patch/delete
use the exact selected ETag. Google's [version contract](https://developers.google.com/workspace/calendar/api/guides/version-resources)
does not grant this library permission to refetch and overwrite after HTTP 412.

Attempted-write failures, including cancellation, preserve unknown outcome.
Cleanup joins entered work. There is no automatic resend, new event ID, backend
switch, rollback or exactly-once guarantee. The host reconciles against the
selected operation identity. Host authentication, hooks and transports are trusted
components and must not replay or retarget requests themselves.

The paired host's existing Calendar dispatch consumes these operations while retaining
internal-calendar raw-write denial, origin stamping, write-version ledger,
semantic publisher, deterministic event IDs and final readback. A typed mutation
failure reaches the existing publisher receipt as `write_outcome: "unknown"`.
The existing best-effort version ledger is not crash-safe fencing and does not
isolate identical event IDs across accounts/calendars. This catalog neither
changes that policy nor certifies complete the paired host adoption.

The retained source checkpoint is
[calendar-transport-adoption/source-checkpoint-1](../../../../../../docs/coordination/20260930-upstream-review/calendar-transport-adoption/source-checkpoint-1/manifest.json):

| Input | SHA256 |
| --- | --- |
| Source checkpoint manifest | `a69810e5c7e82aba27589028724aba6343ec8df383e53a65395f7740aa5ec43c` |
| `google_calendar.py` | `495b71c872af8fb6c53bae0b1e66b5693cf51226109a162510a5d136e1d728ce` |
| `GOOGLE-CALENDAR.md` | `b324a9df44d836b93ff76bdb5c812314313ee60ca60b3d406dbfdd7c203becc3` |

The subsequent [installed private-artifact acceptance](../../../../../../docs/coordination/20260930-upstream-review/calendar-transport-adoption/root-acceptance.json)
binds that source to SDK wheel SHA256
`a2e7ffe92f03d8cf23759891b7a4895e07053bcc45e66b1e0f090d0a0ead9d0e`
and sdist SHA256
`43ebc23c299c01ceaf26a4e52464b90f5bb4080a238ed1c3d68ce30f76f915e1`.
It records installed SDK tests and actual isolated the paired host composition with
Python 3.11.15, HTTPX 0.28.1 and AnyIO 4.15.1. The acceptance receipt itself has
SHA256 `6231daa3b3bb0d649dd73409c361806ce54f51f19ce6970a2aa93f89c827d2e4`.
This is verified local installation evidence for those private bytes. Reverify
other artifacts; no public PyPI release or live Google access is claimed. No
TypeScript Calendar implementation is represented here. Local tests are not
production OAuth, calendar-account isolation or deployed-readiness receipts.

## Source observations and license scope

All five official public pages were fetched on 2026-10-01. `recipes.json` retains
their URL and raw HTTP-body SHA256, joined to package assessment references.
[Source evidence](../../../../../../docs/coordination/20260930-upstream-review/calendar-library-successor/source-observations.json)
retains the actual body paths, HTTP status and observation timestamps. These
hashes identify inspected documentation bytes, not Google's mutable service.

The metadata is original Toolkit work under MIT OR Apache-2.0. Google documentation
states CC BY 4.0 for prose and Apache-2.0 for samples, subject to page exceptions;
hosted services retain their terms. No provider implementation is copied. The
Python SDK artifact's existing mixed-source declaration applies to that artifact;
this metadata does not grant rights over its other private-source modules.

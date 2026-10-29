# Configured Slack, Gmail, Calendar and Drive MCP component

This standalone Python package exposes an explicit subset of Toolkit's existing
Slack, Gmail, Google Calendar and Drive operations over standard MCP stdio. It also supplies host-only
Google and Slack authorization-code helpers, explicit token refresh and a
prepared HTTPX client composition API. These let a Python host use the reviewed components from an
installed wheel without a Toolkit source checkout. They do not add to the
101 integration count, register a native host, discover or install connectors,
own a login ceremony, schedule refreshes, persist tokens or grant access.

The package uses `meerkat_toolkit_services`, so it can coexist with the combined
SDK and the separate Agno component. Its connector bodies and HTTPX cleanup owner
and protocol helpers are copied exactly from the combined SDK. The package
excludes private MIME construction, frontend and native host-control modules.

## Install the supplied wheel

Version `0.1.19` adds opt-in readable Gmail content. `read_message` decodes
available inline text and describes omitted parts; `read_text_part` reads one
explicitly selected text part using the same account and transport. The new
public formatter uses Gmail's JSON contracts and Python's standard library.
It does not include any host's private message formatter.

Version `0.1.18` makes the existing Calendar `insert_event`, `patch_event` and
`delete_event` operations available through explicit configuration. Read-only
selections retain their existing tools. Conditional writes preserve exact ETags;
uncertain outcomes never cause an automatic retry. The same Calendar connector
and account selection are used, with no new integration or credential owner.

Version `0.1.17` adds read-only Drive search, metadata and bounded content
retrieval. It reuses the existing Drive transport and returns exact text or
binary through MCP resources, without adding an integration to the count.
The host-only Google OAuth helper also accepts explicit Calendar and Drive
service selections; omission retains its existing Gmail behavior. See the
packaged `GOOGLE-OAUTH.md` for the fixed scope vocabulary and known-account
reconnect/refresh limits. It does not add a login or token storage owner.

Version `0.1.16` introduced read-only `google-calendar` operations for a host-selected
calendar. The exact existing Calendar transport is included; no dependency or
integration count was added. Calendar discovery, automatic pagination and live
account authentication remain outside this profile.

Version `0.1.15` preserves a finite set of documented token-endpoint refusal
codes in the existing host-only Slack and Google OAuth errors. Hosts can use
that evidence to explain a repair without receiving raw provider diagnostics.
Existing error reasons and exchange state remain unchanged. No automatic
retry, credential persistence or native authentication is added. That component
has since passed installed review and canonical selection.

Version `0.1.14` lets the host explicitly select `users:read` in the existing
Slack known-account OAuth reconnect and refresh helpers. The helper never adds
this scope automatically; email, write and unknown requested scopes remain
refused. A missing required scope still rejects the returned token. This
version was accepted by the preceding independent component review.

Version `0.1.13` added optional `get_user` / `slack_get_user` for one explicitly
selected Slack user profile. It requires host-authorized `users:read`, preserves
provider fields and validates the exact returned user ID. It remains independent
of the channel routing ceiling and does not infer membership or enrich messages
automatically.

Version `0.1.12` added documented Gmail HTTP 403 classification with an optional
safe `provider_reason`. Rate limits return `rate_limited`, daily quota exhaustion
returns `quota_exceeded`, and domain policy returns `permission`. Unknown or
ambiguous reasons retain generic `permission`. No retry, reconnect or broader
consent is performed; every entered write still has an unknown outcome on error.

Version `0.1.11` added fixed, nonsecret configuration diagnostics to explicit
`check-config` failures. Its accepted configurations and successful command
output are unchanged. Diagnostics identify a known validation rule and field
path, without reflecting supplied values or unknown field names.

Version `0.1.10` added a host-only Google access-token refresh observation.
It verifies the known subject through UserInfo using the new access token.
Missing response scopes remain unknown in the result; the native credential
owner must decide whether to accept a candidate under its existing grant.
It does not consume or verify an optional ID token as a new sign-in event.

Version `0.1.9` added host-only Slack public-client token refresh with exact
account and scope observation. The host explicitly invokes this operation and
owns serialization and credential replacement. No refresh tool is exposed to
agents. The unchanged MCP operation interface still reports `0.1.8`.

Version `0.1.8` added optional `allowed_recipient_addresses` for configured Gmail
structured sending. It checks every To/Cc/Bcc recipient before invoking the
connector and refuses raw send/draft operations in restricted configurations.

Version `0.1.7` added an optional fixed Slack channel routing ceiling, enforced
before a provider request. It also exposes the optional typed array through
`config-help`.

Version `0.1.6` made the borrowed-client API and the two stateless OAuth protocol
helpers available in the standalone namespace. Google code-exchange verification
requires the optional `google-oauth` extra; Google refresh, Slack and ordinary
token-based MCP use do not.

Version `0.1.5` added opt-in resumable Slack interval reads. It preserves pending
thread/history work across caller-chosen requests without a new state service.
Version `0.1.4` added the optional `gmail_get_profile` mailbox observation described
below. It does not change send-only setup or request additional provider scopes.
Version `0.1.3` added `gmail_send_message`, accepting explicit message fields
without requiring an agent to construct MIME or base64. It retains the account
and permission guidance from `0.1.2` and Slack permalinks from `0.1.1`. Select a
consumer bundle that explicitly contains
`meerkat_toolkit_services-0.1.19-py3-none-any.whl`; existing bundles retain their
original behavior and guidance. Verify the selected bundle's
`SHA256SUMS` before installation. An offline bundle includes its dependency
wheels and `install-offline.txt`, with exact versions and artifact hashes.
Ordinary pip installs the selected bytes. Installation does not configure an
account or register an MCP server with a host.

```sh
python3.11 -m venv /absolute/new/service-env
cd /absolute/path/services-bundle
/absolute/new/service-env/bin/python -m pip install --no-index --find-links . --only-binary=:all: --require-hashes --requirement install-offline.txt
```

For Google code-exchange signature verification, use the separately supplied
`install-google-oauth-offline.txt` with the same command. It selects the same
service wheel with its `google-oauth` extra and adds the exact Google verification
dependencies. The base install does not install `google-auth`.

The verified consumer uses macOS arm64 and Python 3.11. The component is a pure
Python wheel declaring Python 3.11 or later; other platform/interpreter pairs
have not been verified here. Use the dependency wheels for the selected platform;
a pure Python component can still depend on platform-specific wheels.

`PACKAGE-SOURCES.json` binds exact source and notice bytes. Its guide mapping
records the literal import-namespace and packaging-reference substitutions made
to the shared host guides. It is a packaging
manifest, not a grant, installation receipt or account-readiness assertion.
Restore software by selecting a previously installed isolated environment and its
saved configuration. Software restore does not revoke credentials, undo sends,
remove drafts or reverse any provider effect.

## Inspect without credentials

```sh
/absolute/new/service-env/bin/meerkat-toolkit-services-mcp profiles
/absolute/new/service-env/bin/meerkat-toolkit-services-mcp config-help --profile slack
/absolute/new/service-env/bin/meerkat-toolkit-services-mcp check-config --config /absolute/slack.json
/absolute/new/service-env/bin/meerkat-toolkit-services-mcp describe --config /absolute/slack.json
```

These commands use only Python's standard library and the explicitly selected
configuration file. They do not import provider/MCP/HTTPX runtimes, resolve the
credential reference or contact providers. `config-help` is field guidance, not
a complete JSON Schema. `check-config` validates configuration shape and local
limits; provider permission, identity and readiness remain unverified. Ordinary
Python/argparse platform and locale environment lookups may occur.

An explicit `check-config` failure from a known local validation rule exits
nonzero and emits a bounded diagnostic object on stdout, for example:

```json
{"valid":false,"scope":"configuration_only","diagnostic":{"version":1,"code":"channel_list_conflict","field_path":["operations"]}}
```

This example means that a fixed Slack channel ceiling cannot be combined with
conversation-list discovery. Correct the selected operations while retaining
the host-required ceiling, then validate the complete configuration again.
Diagnostics never authorize broader operations or removal of a restriction.
The code/path pairs are declared by `CONFIGURATION_DIAGNOSTIC_PATHS` in the
configuration owner. They cover document/profile/operation shape, required or
unknown fields, channel and recipient limits, incompatible operations,
credential-reference syntax, numeric bounds and mailbox syntax. Read/decode
failures use `invalid_config_file` with an empty path. A path always uses known
configuration keys; neither unknown keys nor submitted values are returned.
Unknown runtime failures and failures in other modes retain the generic error
without a diagnostic object. Stderr remains the fixed command-failure message.
Consumers must require both the failure exit and the exact diagnostic schema;
a diagnostic is not a valid configuration, installation or account observation.

This edition's `config-help` also returns `prerequisites` and
`unresolved_details`. These distinguish the selected provider credential from
the local MCP stdio transport, explain the host's account and credential
responsibilities, and explain registration, conditional administrator approval
and provider scopes for the supported operations. Slack guidance distinguishes
conversation kinds, membership and rate tiers. Gmail guidance distinguishes
metadata-only reads, content reads, send-only and draft permissions. Help is
profile-wide: follow only the instructions applicable to the chosen operations,
rather than requesting every listed scope. The selected token's actual grants,
account and resource access remain unverified. This guidance uses the existing
library prerequisite vocabulary; it does not start authentication or infer
which permissions a token has.
The earlier `dist/services-consumer-20261003/` artifact predates these additional
help fields. Its frozen guide and evidence remain applicable to that artifact.

This edition also returns `credential_environment_references` after
`check-config` succeeds. Earlier artifacts without this field remain unchanged.
An example is:

```json
{"credential_environment_references":[{"field_path":["config","token_env"],"environment_variable":"HOST_SELECTED_SLACK_TOKEN"}]}
```

`field_path` contains literal JSON keys. Rows derive from the existing help's
credential declarations and the same validated configuration; only reference
names are returned, with exact case and one row per selected field. No value is
looked up or copied from the environment. `profile`, `valid` and
`scope: "configuration_only"` remain unchanged. Use `config-help` for credential
meaning and prerequisites. The host freezes the configuration and owns lookup,
secret handling and protected-environment policy, including whether a name such
as `PATH` may be injected. This field grants no overwrite permission, account
binding, authentication or readiness.

## Select one account and operation subset

Each process selects exactly one profile and an explicit nonempty operation
subset. Unknown configuration keys, raw tokens, provider URL overrides,
duplicate JSON keys and values outside the published bounds are refused.
Slack interval caps are required only when `read_interval` is selected.

```json
{
  "profile": "slack",
  "operations": ["auth_test", "get_conversation", "read_interval"],
  "config": {
    "token_env": "HOST_SELECTED_SLACK_TOKEN",
    "allowed_channel_ids": ["C12345678"],
    "timeout_seconds": 20,
    "max_response_bytes": 2097152,
    "max_interval_requests": 30,
    "max_interval_messages": 1000
  }
}
```

```json
{
  "profile": "gmail",
  "operations": ["list_messages_page", "get_message", "send_message"],
  "config": {
    "token_env": "HOST_SELECTED_GMAIL_TOKEN",
    "user_id": "me",
    "allowed_recipient_addresses": ["reader@example.invalid"],
    "timeout_seconds": 20,
    "max_response_bytes": 2097152,
    "max_message_bytes": 1048576
  }
}
```

The host supplies only the selected access token through the named environment
reference when launching `serve`. Do not place credentials in config files or
command arguments. The adapter resolves that reference once, creates a dedicated
client, and sends Bearer authorization only to the fixed official origin:
`https://slack.com/`, `https://gmail.googleapis.com/`, or `https://www.googleapis.com/`
for Calendar. HTTPX environment proxy,
netrc and certificate configuration are disabled (`trust_env=False`); redirects
and automatic retries are disabled. CLI users cannot select a custom origin.
Gmail `user_id` is explicitly `me` or a simple ASCII email address. This selects
request routing; it does not prove the token belongs to that mailbox.

```sh
/absolute/new/service-env/bin/meerkat-toolkit-services-mcp serve --config /absolute/slack.json
```

Use this executable, these arguments and the host-owned token environment entry
in any standard MCP stdio client configuration. Use a separate process and token
for another account. A running process does not reload environment values or
configuration. The host replaces it to rotate credentials or select new scopes
and operations. Account authorization, refresh, scope choice, user consent,
requester and audience restrictions stay with existing host owners. Tool
annotations are descriptive hints, never authorization enforcement.

`allowed_channel_ids` is optional for Slack. When present, it must be an array
of at most 256 unique exact IDs, each 1-128 ASCII letters or digits. An empty
array denies every channel call; omitting the field leaves channel routing
unrestricted by this adapter. JSON `null` is invalid. The fixed ceiling applies
to metadata, permalinks, history, replies and every resumed interval request.
The configuration cannot combine this ceiling with `list_conversations_page`:
broad discovery has no supported restriction strategy and is refused, without
fetching or filtering a broad response. Selected `auth_test` and `get_user`
remain available. User lookup is independently selected workspace metadata;
the channel ceiling does not constrain it or attest channel membership.

The setup browser accepts this field as a JSON array, and setup MCP accepts an
array value. The host must review and authorize the exact configuration; an
agent's proposed list is not a grant. Replacing the process is required to
change its ceiling. This limit neither narrows the provider token itself nor
filters quoted text, links or other content in messages from allowed channels.

`allowed_recipient_addresses` is optional for Gmail. It accepts at most 256
unique bare mailbox addresses. Omission adds no recipient limit; `[]` denies
every structured send or draft; JSON `null` is invalid. All To, Cc and Bcc recipients must
match. Comparison preserves local-part case and normalizes only ASCII domain
case. It does not infer aliases, remove dots or plus suffixes, interpret wildcards
or equate internationalized domains with their ASCII encodings. Display names
are allowed in message arguments but not in the configured list.

A restricted configuration cannot select `send_raw` or `create_draft_raw`.
These operations accept opaque MIME whose recipients this component does not
interpret. Direct Python and TypeScript raw APIs remain host-authorized APIs
outside this configured-services protection. A recipient refusal reports
`permission`, `operation_invoked: false` and `write_outcome: not_attempted`, with
no recipient or message contents in the error. Accepted calls retain the normal
uncertain-write behavior after connector entry. The host still approves sender,
content and recipients; this fixed limit does not verify account ownership or
prevent recipients forwarding email.

## Operations and effects

| Profile | Selected operation names | Effect |
| --- | --- | --- |
| Slack | `auth_test`, `list_conversations_page`, `get_conversation`, `history_page`, `replies_page`, `read_interval`, `get_permalink`, `get_user` | Read |
| Gmail | `list_messages_page`, `get_message`, `get_profile` | Read |
| Gmail | `send_message`, `create_draft_message`, `send_raw`, `create_draft_raw` | Write |
| Google Calendar (`google-calendar`) | `list_events_page`, `get_event` | Read |
| Google Calendar (`google-calendar`) | `insert_event`, `patch_event`, `delete_event` | Write |

MCP tool names are prefixed with `slack_`, `gmail_` or `google-calendar_`. Only configured operations
are listed and callable. This component has no Slack send, Gmail modification,
attachment-fetching or other hidden dispatch path. The underlying connector
validation still applies, including UTF-8 byte limits, control-character rules
and relationships between arguments. MCP schemas describe individual fields;
they do not replace that validation.

### Calendar reads

The host selects one account, calendar and operation subset. With the read-only
configuration below, a model can choose query parameters, page tokens and event
IDs, but cannot change that calendar or invoke writes.

```json
{
  "profile": "google-calendar",
  "operations": ["list_events_page", "get_event"],
  "config": {
    "token_env": "HOST_CALENDAR_ACCESS_TOKEN",
    "calendar_id": "primary",
    "timeout_seconds": 20,
    "max_response_bytes": 2097152
  }
}
```

Use `config-help --profile google-calendar`, then `check-config` and `describe`
with the selected file before `serve`. These inspection commands need no token.
`primary` is Google's account-relative alias, not an identity check. An explicit
calendar ID is preserved and encoded as one path segment; empty IDs, dot-only
segments, controls and more than 8192 UTF-8 bytes are refused.

Enable the Calendar API and obtain a suitable access token through the host's
existing protected Google OAuth flow. Both reads accept the
`https://www.googleapis.com/auth/calendar.events.readonly` scope. A scope alone
does not grant calendar access, reveal private event details or approve sharing
them. The component neither obtains consent nor broadens scopes. See Google's
[scope guide](https://developers.google.com/workspace/calendar/api/auth),
[list contract](https://developers.google.com/workspace/calendar/api/v3/reference/events/list)
and [get contract](https://developers.google.com/workspace/calendar/api/v3/reference/events/get).

The packaged Google reconnect/refresh helpers accept explicit `service="calendar"`
with their documented Calendar scope vocabulary. Omission still selects Gmail.
The host supplies the known account, protected callback and credential owner;
no agent-facing authentication tool or automatic refresh is added.

For a weekly view, the caller supplies explicit RFC3339 `time_min`/`time_max`
with offsets and usually `single_events: true, order_by: "startTime"`. Google
filters by event overlap: the lower bound applies to event end, the upper bound
to event start, both exclusively. The adapter does not infer the week or time
zone. It preserves offset/timeZone fields, including daylight-saving changes,
and all-day `date` fields with exclusive end dates. Recurrence and minimal
cancelled tombstones stay unchanged; `show_deleted` is an explicit query choice.

`list_events_page` performs one bounded request. `nextPageToken` means coverage
is incomplete, including empty pages. Pass that token as `page_token` with the
same query for the next explicit call. No token ends only that query for that
calendar; it does not prove a complete agenda across calendars or all updates.
`nextSyncToken` is retained but cannot be consumed by this profile. A later page
failure is an error, not a completed or truncated success; retain prior pages
and report the coverage gap. Missing optional fields remain absent. No event
projection, sorting, timezone conversion or synthetic completion flag is added.

### Calendar writes

Explicitly add `insert_event`, `patch_event` or `delete_event` to the selected
`operations` only when the host authorizes that operation. The Calendar ID stays
fixed. The existing Google account must have write access to that calendar and
a suitable scope, such as `https://www.googleapis.com/auth/calendar.events`.
The narrower `calendar.events.owned` and `calendar.app.created` scopes apply to
their respective restricted sets of calendars. Read-only scopes cannot write;
the component does not request or broaden them.

`insert_event` accepts `{"event": {...}}`. Supply the intended event JSON,
including start/end and any host-selected stable ID. `patch_event` accepts
`{"event_id": "...", "patch": {...}, "expected_etag": "\"...\""}`;
`delete_event` accepts the same ID and optional ETag without a patch. A quoted
ETag is sent unchanged as `If-Match`. Omission or JSON `null` explicitly requests
an unconditional operation. Wildcards and ETag lists are refused. A conflict
does not fetch a newer version or overwrite it automatically.

Bodies retain omission, null, false, empty strings and arrays. Patch arrays
replace the existing array. The existing connector bounds each serialized JSON
body at 2 MiB before HTTP dispatch. The provider validates event fields, and no
time-zone conversion or event defaults are inserted. All-day end dates remain
exclusive. The full event receipt is returned for insert/patch; deletion returns
`null` only after an empty HTTP 204 response.

Notification query options, attachment support and conference-creation options
are not exposed. Provider defaults apply, and email may still be sent. The host
must authorize the event and affected attendees; a successful receipt is not
notification delivery. Once the connector is invoked, errors conservatively
report `write_outcome: unknown`, including conflicts, timeouts and malformed
receipts. Cancellation joins entered cleanup and cannot undo a provider effect.
Do not automatically retry, choose a replacement ID or treat a missing event as
successful deletion. Reconcile through the host's selected account and operation.

Contracts checked 2026-10-04: [insert](https://developers.google.com/workspace/calendar/api/v3/reference/events/insert),
[patch](https://developers.google.com/workspace/calendar/api/v3/reference/events/patch),
[delete](https://developers.google.com/workspace/calendar/api/v3/reference/events/delete)
and [conditional versions](https://developers.google.com/workspace/calendar/api/guides/version-resources).

Select provider scopes with the existing host auth flow. Slack channel discovery
and history access depend on token type, granted conversation scopes and actual
membership. `auth_test` returns observed identity for the host to compare;
conversation metadata is not an audience attestation. Gmail listing/getting,
sending and draft creation require the corresponding provider permissions; a
metadata-only Gmail scope does not support query filtering. A local operation
allowlist cannot narrow the provider token itself.

For author or mention names, explicitly select `get_user` and call
`slack_get_user {"user":"U123","include_locale":false}`. The user ID is required;
the boolean locale flag defaults to false. This makes one `users.info` request
and preserves the full response, including absent, null and unknown fields,
after validating the exact returned user ID. `user_not_found` is a safe
`not_found` error. There is no cache or automatic history enrichment. The host
must authorize `users:read`; names do not require `users:read.email`, and no
scope is requested automatically. A profile is independent workspace metadata,
not proof of channel membership. See
[Slack users.info](https://docs.slack.dev/reference/methods/users.info/).

To observe the configured Gmail mailbox, explicitly select `get_profile` and
call `gmail_get_profile {}`. It makes one request to the configured `user_id`
route and returns Google's profile, preserving `emailAddress`, any message/thread
counts, `historyId` and additional fields. The host can compare this observation
with the intended mailbox. It does not verify an OAuth issuer/subject, granted
scopes, send-as permission or a Toolkit grant, and it never changes `user_id` or
the sender of a message. The tool has no account-selection argument.

The [profile endpoint](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users/getProfile)
accepts `gmail.metadata`, `gmail.readonly`, `gmail.compose`, `gmail.modify` or
`https://mail.google.com/`; **`gmail.send` alone is insufficient**. Do not add a
broader scope merely because this optional tool exists. Use it when the host
has independently authorized a suitable scope. Account verification for a
send-only token remains with the host's authentication owner. Gmail email
addresses are not the stable issuer/subject identifiers used by
[Google OpenID Connect](https://developers.google.com/identity/openid-connect/openid-connect).
These references were checked on 2026-10-04.

Neither startup, tool discovery nor sending calls `get_profile` automatically.
A refused profile read is reported without retry, token refresh, fallback,
scope escalation or hidden mail access. Email and history strings are preserved
without normalization, bounded to 1024 UTF-8 bytes and reject control characters.
Optional message/thread counts must be nonnegative integers; omitted fields
remain omitted. This is an observation through the existing response-size and
cleanup owner, not an authentication ceremony.

### Readable Gmail content

Select `read_message` and/or `read_text_part` explicitly in the existing Gmail
configuration. Existing selections and raw `get_message` are unchanged. These
operations use full message content and need content-read permission, such as
`https://www.googleapis.com/auth/gmail.readonly`; `gmail.metadata` is insufficient.
No permission is requested or broadened by the component.

`gmail_read_message {"message_id":"MESSAGE_ID"}` makes one full-message request.
It returns `message` (outer provider metadata excluding payload/raw), exact
ordered outer `headers`, and `parts`. Every part retains its tree `path`, opaque
`partId`, `mimeType`, filename, selected charset, disposition, attachment ID and
declared size, plus explicit attachment/outer-body/embedded-message flags. Plain
and HTML alternatives remain separate. HTML is source text, never rendered,
stripped or followed. Nested `message/rfc822` headers and bodies cannot become
the outer sender or text. Attachments are descriptors, even when their bytes
are inline in the provider response.

Available outer text is decoded into `text`. `omitted_reason` distinguishes
containers, attachments, nontext, embedded messages, external bodies, missing
bodies, unsupported charset and invalid text. `body_complete` is conservative:
there must be an outer text part and no omitted non-attachment outer content.
It never asserts that attachments were read, that HTML assets were fetched or
that the sender was authenticated. Known empty text is `""`; missing data is
not silently treated as empty.

`gmail_read_text_part {"message_id":"MESSAGE_ID","part_id":"PART_ID"}` first
re-reads the full message, then resolves exactly one part. The empty string is
valid for the root part when the provider supplies that ID. Only a text/plain
or text/html leaf may be selected; duplicate IDs, embedded message descendants
and nontext parts refuse. If that exact part uses an external attachment ID,
the operation makes one additional attachment request and validates its size
against the fresh message metadata. A caller cannot supply an arbitrary
attachment ID or URL. Explicit text attachments are allowed and stay labelled
as attachments. The result retains the outer `message` metadata, selected `part`
and `body_source` (`inline` or `attachment`). It is a current read, not a pinned
historical MIME snapshot or complete-message result.

One configured timeout covers the complete one/two-request operation, including
joined response cleanup. Cancellation and provider failures preserve the
existing request owner's behavior; no retry, caching or background fetch occurs.
Each provider response remains subject to `max_response_bytes`. That same bound
also limits the formatter's input/output JSON projection and aggregate decoded
inline bytes. Additional limits are 32 nesting levels, 1,000 parts, 2,000 headers
and 256 KiB of UTF-8 header names/values. The JSON projection limit includes
escaping and may refuse an input that fit the raw HTTP limit. It does not claim
to bound the entire duplicated MCP envelope. Limits refuse without clipped text.

Supported charsets are ASCII/us-ascii, UTF8/utf-8, Latin1/latin-1/iso-8859-1 and
CP1252/windows-1252. Missing charset uses ASCII. Charset decoding is strict;
unsupported or invalid text is an explicit omission in `read_message` and a
`content_error` in `read_text_part`. Malformed MIME structure, conflicting
content headers, invalid canonical base64url or a mismatched supplied body size
also returns `content_error` with a fixed `content_code`, not provider text.
The Gmail API's base64url layer is decoded once; original
Content-Transfer-Encoding is not applied again. RFC 2231 charset parameter
extensions and other charsets are not supported by this profile.

The implementation is new original Toolkit code based on the
[Gmail message/part contract](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages),
[body contract](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages.attachments),
[get](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/get),
[attachment get](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages.attachments/get),
[format scope restrictions](https://developers.google.com/workspace/gmail/api/reference/rest/v1/Format),
[MIME charset defaults](https://datatracker.ietf.org/doc/html/rfc2045#section-5.2),
and Python's standard-library email/base64 APIs, checked 2026-10-04. No private
host formatter code or third-party implementation was copied.

`slack_read_interval` requires explicit `start`, `end`, `root_oldest`,
`max_requests` and `max_messages`. Requests above the configured caps are rejected
before any provider call, never silently reduced. The creation interval is
`start <= message timestamp < end`; root discovery is independently bounded by
`root_oldest <= start`. Replies to older undiscovered roots, deleted/invisible
messages and provider pagination changes prevent blanket completeness claims.
Preserve `coverage_gaps`, cursor state and termination in the result. `finished`
does not mean all workspace updates. The timeout applies to each provider request,
not the entire multi-request interval. A host may cancel an interval; already
made reads cannot be undone.

Each interval result also has a complete-result byte budget. Optional
`config.max_interval_result_bytes` defaults to 262144 and accepts 4096-16777216.
It caps the optional tool argument `max_result_bytes`, which defaults to that
configured value. The budget measures the complete compact ASCII JSON result,
including messages, gaps, chain metadata and continuation, not only message text.
It is independent of each provider response's `max_response_bytes` limit.

`result_budget` admits none of the page that would exceed this limit. With
resumable reads, the returned checkpoint retains that exact pending page. Keep
all previously returned observations and explicitly continue. If no page fits,
increase the allowed byte budget or start a new narrower read with a smaller
page limit. A changed page limit cannot reuse the old checkpoint. A single large
message may remain unavailable under the host's ceiling; disclose that gap.
No content is clipped and no retry happens automatically.

MCP retains the full text and structured representations for compatibility. With
a configured result cap B, their complete serialized `CallToolResult` is bounded
by 3B plus the fixed MCP envelope, excluding transport framing and request ID.
The adapter checks the actual serialized object as well. This makes the default
result less than 1 MiB including both representations. A violated internal bound
returns a content-free `result_too_large` read failure, never a complete result.

To continue after a request budget or rate limit, opt into `resumable: true`.
Supply the returned nonnull `continuation` on the next authorized call with the
same selection. Request/message budgets can change; channel, interval bounds,
page limit and metadata selection cannot. Each part declares
`observation_scope: "current_call_only"`; retain all earlier messages and gaps.
History absent from the resumed traversal queue is `not_requested`. Null continuation means
no pending traversal, not complete coverage. No retry or sleep is automatic.

The checkpoint is transparent, untrusted query data, not account identity or
proof of prior reads. Keep it with the same host-selected account. It contains
only selection, pending provider cursors, seen cursors and discovered root IDs.
It is bounded to 1024 roots, 1025 pending steps, 2048 total seen cursors and
262144 compact UTF-8 JSON bytes. `continuation_budget` refuses a page's entire
projection before it would overflow, returning the prior checkpoint. Narrow
the query as a new read or select raw page operations when capacity is exhausted.
A refused message-budget page can be tried with a larger permitted budget.
Cancellation or response loss provides no new acknowledged progress and a retry
may repeat reads. There is no durable or exactly-once checkpoint guarantee.

Version `0.1.1` adds `slack_get_permalink` for source-linked summaries. It
requires the exact `channel` and decimal-string `message_ts`, makes one
`chat.getPermalink` request and preserves the provider's returned HTTPS URL,
including thread query parameters. It does not construct or open the link,
fetch message contents, or attest that an email recipient can access it. A link
lookup failure must remain visible rather than becoming an invented URL.
Version `0.1.0` wheels and their catalogs do not contain this operation; their
original installation and operation evidence remains scoped to those bytes.

`gmail_send_message` accepts an explicit sender, one or more To recipients,
subject and plain-text body. Optional Cc and Bcc lists contain one mailbox per
entry. Display names and Unicode are supported. For example, after selecting
`send_message`, pass these MCP arguments:

```json
{
  "from_address": "Summary sender <sender@example.invalid>",
  "to": ["reader@example.invalid"],
  "subject": "Last week's Slack updates",
  "body_text": "The reviewed summary and its source links go here."
}
```

The host must authorize the sender, every recipient and content. The sender is
never inferred from the token or `user_id`; Gmail still determines which sender
identities the account may use. This operation uses the same send-only provider
permission as `send_raw`. It does not read the account profile or request broader
inbox permissions. See Google's [sending guide](https://developers.google.com/workspace/gmail/api/guides/sending)
and [send reference](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/send).

Composition creates deterministic plain-text SMTPUTF8 MIME, normalizes body line
endings to CRLF and adds a final text newline. It adds no Date, Message-ID,
attachment or reply threading. Literal subject text is preserved, including text
that resembles encoded headers. Header injection, malformed mailboxes and
oversized input or serialized MIME are refused before sending. Both the summed
UTF-8 input and final MIME must fit `max_message_bytes`; MIME encoding consumes
part of that limit. The existing sender makes one request, with the same
cancellation and uncertain-outcome behavior as `send_raw`.

`gmail_create_draft_message` accepts the same fields and uses the same composer
to create one Gmail draft for review, without sending it. Select
`create_draft_message` explicitly in `operations`; it does not enable any send
or inbox-read operation. The host's `allowed_recipient_addresses` ceiling also
applies to every To, Cc and Bcc recipient of a structured draft. For example:

```json
{
  "from_address": "Summary sender <sender@example.invalid>",
  "to": ["reader@example.invalid"],
  "subject": "Summary for review",
  "body_text": "The proposed summary and its source links go here."
}
```

Draft creation requires a suitable host-selected provider scope such as
`https://www.googleapis.com/auth/gmail.compose`; `gmail.send` alone is
insufficient. The adapter does not request or widen scopes. This scope also
permits sending at the provider, but selecting the structured draft operation
does not expose sending through this MCP profile. The host must authorize the
sender, recipients and content before creation.

The operation makes one draft-creation request through the existing transport
and returns the provider's draft ID and contained message receipt. It does not
update or send an existing draft, add attachments or reply threading, or
accept a `thread_id`. A receipt means a draft was created, not that an email was
delivered. Failure or cancellation after connector entry can leave an unknown
creation outcome; never automatically retry or create a replacement draft.
See Google's [draft creation reference](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.drafts/create).

For `send_raw` and `create_draft_raw`, `mime_base64` is canonical standard base64 of the exact MIME
bytes approved by the host, including every decoded To/Cc/Bcc recipient and all
content. No header is inferred, normalized, repaired or added. Optional thread
routing does not create reply headers. `send_raw` sends once; `create_draft_raw`
creates one draft without sending it. A provider receipt is not recipient
delivery. Never automatically retry either operation after an uncertain outcome.

## Read-only Google Drive

Select one account's access token and an explicit operation subset:

```json
{
  "profile": "google-drive",
  "operations": ["list_files_page", "get_file", "download_content", "export_content"],
  "config": {
    "token_env": "HOST_DRIVE_ACCESS_TOKEN",
    "timeout_seconds": 20,
    "max_response_bytes": 2097152,
    "max_content_bytes": 2097152
  }
}
```

Inspect with `config-help --profile google-drive`, then `check-config` and
`describe` before `serve`. The account comes from the host credential, not a
model-selected account name. There is no folder/file routing ceiling. Enable
the Drive API and use `drive.metadata.readonly` for metadata-only operations,
`drive.readonly` for reading content, or `drive.file` for the app's already
selected per-file access. These names abbreviate
`https://www.googleapis.com/auth/`. `drive.file` includes provider writes but
this profile does not expose them; metadata-only access does not download files.
Scopes do not override account access, administrator policy or download
restrictions. The host-only OAuth helper can explicitly select `service="drive"`;
its protocol result does not itself grant native readiness or persist a token.
See the [Google scope guide](https://developers.google.com/workspace/drive/api/guides/api-specific-auth).

`list_files_page` accepts Drive `query` syntax, `page_size` (1-1000), `order_by`
and `page_token`. It returns exactly one provider page using a fixed field mask:
`nextPageToken,incompleteSearch,files(id,name,description,mimeType,modifiedTime,size,webViewLink,shortcutDetails,capabilities)`.
`get_file` reads the same file metadata by ID. The fixed masks retain useful
source links and download/shortcut metadata, but do not expose every Drive
metadata field. Other returned provider fields remain unchanged. Include
`trashed = false` in the query when desired. Inspect `nextPageToken` and pass
it explicitly with the same query. Even an empty page can continue;
`incompleteSearch=true` means results are missing. No token ends only that query.
A later failure does not invalidate already returned pages or prove completeness.
No full inventory, explicit shared-drive corpora, shortcut traversal or revision
snapshot is promised. See [files.list](https://developers.google.com/workspace/drive/api/reference/rest/v3/files/list).

Choose `download_content(file_id)` for a stored blob or
`export_content(file_id, mime_type)` for a Google Workspace document, such as
`text/plain` or `application/pdf` when supported by that document type. Inspect
metadata and `capabilities.canDownload` first. Neither call auto-follows a
shortcut, changes format after a refusal, downloads a URL, writes a local file,
retries or truncates. Google imposes export-specific limits in addition to the
host bound. See [files.get](https://developers.google.com/workspace/drive/api/reference/rest/v3/files/get)
and [files.export](https://developers.google.com/workspace/drive/api/reference/rest/v3/files/export).

`max_response_bytes` bounds JSON metadata; `max_content_bytes` (1-16777216)
bounds complete raw content. Over-limit, malformed, truncated or encoded HTTP
bodies fail without a partial resource. Success contains a short metadata text
block, `structuredContent.value` with file ID, byte count, provider media type,
representation and resource URI, and an embedded MCP resource. UTF-8 textual
media uses exact text when JSON encoding is no larger than base64. Binary,
other character sets, invalid UTF-8 or escape-heavy text uses exact base64 bytes.
Missing Content-Type becomes `application/octet-stream`; no format is guessed.
The `gdrive:` resource URI identifies this response, not a remotely fetchable
URL or a durable saved resource. The payload is returned once; base64 expansion
is at most `4 * ceil(raw_bytes / 3)` plus bounded metadata and JSON framing.

Binary delivery is not document extraction or a promise that a selected model
can read that media. The source/installed MCP stdio tests exercise a 16 MiB
binary response with the pinned Python client; hosts and models may have lower
message/context limits and should select smaller bounds. Content and preceding
metadata are separate observations and are not bound to one revision.

## Results, cancellation and cleanup

Metadata and ordinary service success wrap the unchanged connector result as
`structuredContent.value`. Drive content uses the resource representation above. Safe
errors contain kind, effect, `operation_invoked` and `write_outcome`, with only
allowlisted provider status metadata. Pre-invocation argument rejection reports
`not_attempted` for writes. After the connector is invoked, write failures are
conservatively `unknown`, including response validation or serialization errors.
`operation_invoked` means the adapter called the connector, not proof the provider
received a request. The adapter does not emit credentials or raw error bodies.

Gmail HTTP 403 diagnostics expose only one recognized `provider_reason`:
`rateLimitExceeded` or `userRateLimitExceeded` (`rate_limited`),
`dailyLimitExceeded` (`quota_exceeded`), or `domainPolicy` (`permission`).
The first three require the provider's `usageLimits` domain; the last requires
`global`. The optional JSON read is bounded to the smaller of the configured
response limit and 65,536 bytes under the original deadline. It requires
`error.code=403` and 1-32 entries with the same recognized domain/reason pair.
Mixed, unknown, malformed, encoded, oversized or interrupted non-cancellation
responses retain `permission` with no reason. Cancellation and total deadline
expiry retain their existing behavior. Neither a reason nor Retry-After
authorizes another send, credential change or broader consent.

Each selected profile admits at most four MCP calls: one executing and three
waiting. Servers borrowing the same profile share that capacity. Admission
precedes argument copying, validation and MIME composition. Excess calls return
`server_busy`; a call still waiting after 30 seconds returns `queue_timeout`.
Both are pre-entry refusals (`operation_invoked=false`), with `not_attempted`
for a write, and neither causes an automatic retry. The queue deadline ends
before execution and leaves the existing provider operation budget intact.

Calls through a profile are serialized. Listing and protocol control remain available
while a provider request runs. Cancellation joins the existing request/response
cleanup owner before the next call enters. An entered Gmail write retains its
unknown outcome under cancellation; an MCP client that cancels may receive no
tool result and must still treat the write as uncertain. Process/transport loss
likewise cannot prove a write did not occur. Closing a CLI-owned profile waits
for calls, closes its client once, and refuses new calls. No work is detached for
later use.

Python hosts can explicitly pass a trusted `client_factory` to `create_profile`
for their own transport (also used by offline fixtures). It must preserve the
provided account, origin and headers and must not retry, retarget or detach
requests. This is a host integration seam, never a CLI configuration option.

## Host-only protocol and prepared-client APIs

The installed wheel includes these modules:

```python
from meerkat_toolkit_services.adapters.services_mcp import create_profile_from_client
from meerkat_toolkit_services.auth import google_oauth, slack_oauth
from meerkat_toolkit_services.httpx_client import prepare_httpx_client
```

`create_profile_from_client` borrows a host-prepared client for the existing
selected profile. It does not resolve `token_env` or create an authentication
flow. The real configuration reference remains present but unused on this path.
The client must have the exact fixed service origin; the host owns credentials,
rotation policy, transport and eventual client closure. Closing a profile closes
admission and joins its calls without closing a borrowed client.

The protocol helpers operate only after a host owner has admitted a known-account
code exchange or a supported refresh. They return private token material and
explicit identity/scope observations for that owner. Google code exchange validates signed identity tokens;
Google refresh instead observes the known subject using the new access token.
It preserves omitted response scopes as missing evidence, never as fresh grants.
The Slack helper binds the selected user token to authenticated workspace/user
observations. Neither opens a browser, owns callback state, commits credentials,
schedules refreshes, chooses tools or authorizes provider effects. They are not MCP
tools and are not invoked by CLI startup or tool discovery. Native integration
remains subject to the generic connector contract tracked in #1631.

Read the installed guides for exact inputs, supported redirect profiles,
scope evidence, deadlines and cleanup:

```python
from importlib.resources import files
guides = files("meerkat_toolkit_services")
print(guides.joinpath("SERVICES-HOST-CLIENT.md").read_text())
print(guides.joinpath("GOOGLE-OAUTH.md").read_text())
print(guides.joinpath("SLACK-OAUTH.md").read_text())
```

All packaged Toolkit modules use the included MIT/Apache-2.0 licenses. The
optional verifier is a separate distribution with its own installed license
metadata; its code is not vendored into this wheel. Local fixture verification
does not establish live account access or complete the native Slack-to-email
journey.

## Maintainer source build

To rebuild from a reviewed Toolkit checkout, stage exact selected modules into a
new directory. Staging performs no installation, credential lookup or downloads.
The builder must supply the exact backend version from `pyproject.toml`.

```sh
python3.11 sdk/services/stage.py /absolute/new/service-source
python3.11 -m pip wheel --no-deps --no-build-isolation --wheel-dir /absolute/new/artifacts /absolute/new/service-source
```

Build results are candidates until their source mapping, package contents and
consumer behavior have been checked. The shared connector source files remain
the implementation owners; this staging path preserves their exact module bytes.

# Gmail operations

Before sharing a client, the host must call `prepare_httpx_client(client)` once.
See [HTTPX client preparation](HTTPX-CLIENT.md) for the setup and ownership contract.
All client arguments in the examples below are already prepared.

Install `meerkat-toolkit[gmail]` and import `GmailOperations` from
`meerkat_toolkit.connectors.gmail`. This Python 3.11+ library requires HTTPX
0.28.x and AnyIO 4.13+, without requiring MobKit, a browser, Google SDK credentials or the Toolkit UI.

```python
from meerkat_toolkit.connectors.gmail import GmailOperations

async def read_one_page(authenticated_client):
    gmail = GmailOperations(authenticated_client, user_id="me")
    return await gmail.list_messages_page(query="label:inbox", max_results=50)

async def send_approved_message(authenticated_client, approved_mime: bytes):
    # The host has approved this account, every recipient and the exact bytes.
    gmail = GmailOperations(authenticated_client, user_id="sender@example.com")
    return await gmail.send_raw(approved_mime)
```

The host supplies an authenticated `httpx.AsyncClient` with a selected API base
URL, normally `https://gmail.googleapis.com/`. The host owns client closure,
credentials, refresh/revoke, scopes, TLS/network configuration, authorization and
any per-invocation approval. `user_id` is required: `me` uses the authenticated
account, while an explicit address selects that mailbox path. This library does
not verify that a credential belongs to the requested mailbox or grant delegated
access. The supported address profile is ASCII letters, digits, `.`, `_`, `+`,
`-` before `@` and ASCII letters, digits, `.` and `-` afterward; unusual address
syntax is refused, not normalized. Separate accounts need separately selected
host authentication. Optional constructor `auth` and `base_url` select explicit
per-request HTTPX authentication and an API base; omitted values retain the
client defaults. They do not mutate a shared client. Do not mutate the selected
authentication while an operation is in flight.

This is an explicit REST strategy. It is never a fallback from unavailable or
denied MCP tools. Google's [Gmail MCP preview](https://developers.google.com/workspace/gmail/api/guides/configure-mcp-server)
has separate prerequisites and tool coverage. Selecting REST is a host/agent
choice requiring its own authorized credential and operation scope.

## Supported operations

- `list_messages_page(query=None, label_ids=(), page_token=None, max_results=100,
  include_spam_trash=False)` performs one GET. It preserves the entire response,
  message order, repeated entries, absent fields, `nextPageToken` and
  `resultSizeEstimate`. The estimate is not a total-count proof. A token is opaque
  data; the caller decides whether another page is authorized and within its
  budget. There is no automatic paging, deduplication or complete-retrieval claim.
  `max_results` is 1-500. Query and token values are limited to 8 KiB UTF-8 without
  control characters; label lists are limited to 100. Gmail's `gmail.metadata`
  scope does not permit `query`. The module never broadens scopes or removes a
  failing query. See [messages.list](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/list).
- `get_message(message_id, format="full", metadata_headers=())` supports `full`,
  `metadata`, `minimal` and `raw`. It preserves nested MIME parts, repeated header
  entries, attachment references and unknown provider fields; it does not fetch
  attachment bodies or render/decode email. `metadata_headers` is accepted only
  with `metadata` and at most 100 header names. A successful response must match
  the selected message ID. See [messages.get](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/get).
- `send_raw(mime: bytes, thread_id=None)` base64url-encodes the exact nonempty immutable bytes and
  sends one JSON POST. It adds no Message-ID, recipient, header
  or retry key. An explicitly supplied `thread_id` is sent exactly. The host must provide valid RFC 2822 MIME and authorize the
  complete bytes, including all To/Cc/Bcc recipients. This module checks the
  bytes' type and length, not MIME validity or recipient policy. Google supplies
  the MIME validation. See [sending](https://developers.google.com/workspace/gmail/api/guides/sending)
  and [messages.send](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/send).

- `create_draft_raw(mime: bytes, thread_id=None)` creates one draft using the same
  exact byte contract. It returns a Draft with a nonempty draft ID and nested
  Message receipt. It never sends the draft. See [drafts.create](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.drafts/create).
- `modify_message(message_id, add_label_ids=(), remove_label_ids=())` sends one
  label update for that exact message. Each supplied list is limited to 100
  identifiers; at least one addition or removal is required. The returned
  Message must match the selected ID. Classification-label administration is
  unsupported. See [messages.modify](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/modify).
- `get_attachment(message_id, attachment_id)` reads one MessagePartBody. Its
  nonnegative integer `size` must match the decoded base64url `data`; zero-byte
  bodies are valid. Unknown fields are preserved. The module does not cache,
  save, convert or execute attachments. See [attachments.get](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages.attachments/get).

Optional thread routing is not a guarantee of thread membership. Google also
requires matching Subject and valid References/In-Reply-To headers. The caller
supplies those exact MIME bytes; this module does not repair them. See the
[threading contract](https://developers.google.com/workspace/gmail/api/guides/threads).

Results are provider dictionaries, including unknown fields; their nested
containers remain mutable. Message/get/send results require nonempty `id` and
`threadId` strings. This is a bounded response profile, not validation of every
future Gmail field. A send result is a provider receipt, not proof that a
recipient received or read the message. A Message-ID is not an idempotency key.
Searching for it may inform host reconciliation but cannot prove an uncertain
send did not occur. Never automatically resend because a search returned empty.

## Transport and failure contract

The client object, base URL and selected account route are captured at
construction. Changing `client.base_url` later cannot retarget this instance;
an optional reverse-proxy prefix is retained. Userinfo, query and fragment in
the base are refused. HTTP(S) bases are supported for explicitly selected hosts
and loopback tests; the host must select the correct credential audience and
secure production transport. There is no caller-supplied per-operation URL.
Message/thread/attachment/label identifiers use ASCII letters, digits, `_` and `-`, at most 1024
characters. Account path encoding and operation query encoding are explicit.

Client default query parameters are excluded; redirects are refused even when
the client normally follows them. The library requests identity encoding and
accepts only identity-encoded `application/json`. Duplicate JSON keys,
non-finite numbers, malformed JSON and invalid operation response shapes fail.
The default limits are 2 MiB response bytes and 4 MiB unencoded MIME bytes;
constructor options can change them. The latter is this library's profile,
not a statement of Gmail's maximum upload size. Media/resumable upload, draft-send, history/watch and arbitrary query/body fields
are outside this profile.

One operation deadline (20 seconds by default) covers send and the entire
streamed response. Expiry starts response closure, which is awaited. This is
not a promise that arbitrary host transport cleanup completes before the
operation deadline. Each request has one retained task and cancellation scope. Repeated caller
cancellation cancels that scope and joins the actual request and shielded response
closure before returning; no cleanup is detached and no retry is created. Host
HTTPX auth, hooks, proxy and transport implementations must not retarget or
replay requests; they are opaque host-owned behavior. A normal HTTPX transport
has no automatic retry loop. This library does not freeze mutable client
credentials or enforce requester, resource or cross-user policy.

`GmailError` exposes `kind`, optional HTTP status, `write_outcome`, an
optional decimal `retry_after_seconds`, and optional `provider_reason`.
HTTP-date Retry-After is not parsed.
Provider bodies, queries, MIME, URLs and underlying exceptions are omitted from
error text and exception chaining. Returned data is private provider content;
host hooks, logs, debuggers and traceback-local inspection remain the host's
responsibility.

The Python connector distinguishes four documented HTTP 403 reasons. Exact
`usageLimits` reasons `rateLimitExceeded` and `userRateLimitExceeded` return
`rate_limited`; `dailyLimitExceeded` returns `quota_exceeded`. Exact `global`
reason `domainPolicy` returns `permission`. Only that allowlisted reason string
is exposed. See Google's [Gmail error contract](https://developers.google.com/workspace/gmail/api/guides/handle-errors).

This optional diagnostic read is capped at the smaller of `max_response_bytes`
and 65,536 bytes, under the original request deadline and response owner. It
requires JSON with integer `error.code=403` and 1-32 error entries that all name
the same recognized domain/reason pair. Mixed, unknown, malformed, encoded,
oversized or interrupted non-cancellation bodies preserve generic `permission`
without a reason. Other statuses retain their existing handling. Total deadline
expiry and caller cancellation retain the existing timeout/cancellation and
joined cleanup behavior. A rate or quota diagnostic does not authorize a retry,
reconnect, broader consent or a different account. This is current Python source
behavior; previously built component artifacts retain their original bytes.

Reads use `WriteOutcome.NOT_APPLICABLE`. Local validation raises `ValueError`
before the send attempt. Once send, draft creation or label modification enters the HTTP operation, every failure
is `WriteOutcome.UNKNOWN`, including HTTP refusal, host hook failure, redirect,
malformed success, oversized result, timeout or connection loss. This is a
conservative failure contract, not a claim that all failures had an effect.
Cancellation of every entered write remains cancellation through the compatible
`SendCancelled` name, an
`asyncio.CancelledError` subclass carrying the same unknown outcome. Cancellation
does not undo remote work. No retry permission follows from an error or
Retry-After. The business invocation/reconciliation owner retains that decision.

## Provenance and evidence boundary

The `connectors.gmail` REST transport is original MIT OR Apache-2.0 Toolkit code
based on Google's linked public REST contract, reusing this repository's HA
transport discipline. No Google, Agno or the paired host implementation is copied into
that transport. The separate `connectors.gmail_message` formatting module is
PairedHost-derived source licensed under MIT OR Apache-2.0 for the extracted
Toolkit copy, as recorded below and in `LICENSE-EXTRACTED`. Google documentation states CC BY 4.0
for prose and Apache-2.0 for samples; hosted Gmail retains its own terms. HTTPX and AnyIO
are optional dependencies for this profile. Google Cloud project, Gmail API enablement,
OAuth consent/client registration, scopes and administrator restrictions remain
host setup prerequisites, not capabilities supplied by this module.

`tests/test_gmail.py` and `tests/test_gmail_extended.py` use actual loopback HTTPX sockets and the existing retained
HA fixture owners. It exercises pagination continuation without completeness
claims, representation fidelity, same-endpoint account separation, exact send
bytes, refusal, response limits, cancellation, timeout, connection loss and
cleanup after body failure. These are protocol fixtures, not a live Google
server, authenticated user flow, native OAuth integration, exactly-once
business delivery, package activation or the paired host migration. M4/M8 and all
other complete end-state requirements remain open.

The paired host routes its existing Gmail command shapes through these operations
using its existing broker. Host composition tests are local protocol evidence;
they do not establish live Google behavior or native approval completeness.

## Pure message formatting

`meerkat_toolkit.connectors.gmail_message` is a separate stdlib-only surface.
It does not create a client, fetch attachments, approve, send, retry, render HTML
or store anything. Its frozen records carry provider-format facts, not authority.
`examples/gmail_message.py` demonstrates explicit decoding and reply construction.

`decode_message(full_message, default_charset="us-ascii", limits=GmailDecodeLimits())`
returns an immutable selected view. `headers` preserves the ordered outer header
pairs, including duplicates; each part keeps its own header pairs, preorder
`path`, exact `part_id`, filename, MIME type, attachment reference and size.
This selected view is not raw-message preservation. Use unchanged
`GmailOperations.get_message(..., format="raw")` for the raw response.

Named parts, `Content-Disposition: attachment`, and `message/rfc822` establish
attachment boundaries. Their descendants cannot supply the outer body or sender.
Available outer text/plain and text/html bodies are decoded without HTML rendering.
A genuine empty text body yields `text=""`. Missing data without empty proof,
external text references and unsupported outer body types instead set `complete=False`
and a per-part `unavailable_reason` (`missing_body_data`, `external_body` or
`unsupported_body`). This is not complete empty success and no implicit fetch follows.
Attachments are only descriptors, even if provider bytes are present.

The supported charset names are ASCII/us-ascii, UTF8/utf-8,
Latin1/latin-1/iso-8859-1 and cp1252/windows-1252. An explicit Content-Type charset
wins. The public default is ASCII; the paired host explicitly uses UTF-8 to retain its
previous undeclared-charset profile. Undecodable or unsupported text refuses
without replacement. Gmail `body.data` is canonical base64url (correct padding
optional), decoded once; Content-Transfer-Encoding is not applied again. A
supplied decoded `size` must match available bytes; empty data alongside an
attachment reference describes unavailable bytes rather than a zero-sized file.
Duplicate Content-Type/Content-Disposition, conflicting MIME declarations,
nonempty inline data plus an attachment reference, and inline multipart bytes
refuse rather than silently discard content.

Default limits are depth 32 (root depth 0), 1000 parts, 2000 headers, 256 KiB
UTF-8 header bytes and 2 MiB decoded inline bytes. Limits can be lowered or raised
explicitly. All available decoded bytes count, including attached bytes. Cycles,
invalid shapes and exceeded bounds refuse the whole decode; there is no clipped
partial-success object. Reused acyclic nodes are counted at each occurrence.

`validate_reply_selection(parent_id, thread_id, subject)` checks explicit choices
before the host fetches the parent. `reply_linkage(parent, parent_id=...,
thread_id=..., subject=...)` then checks exact IDs, thread and decoded Subject.
It retains the existing bounded simple ASCII Message-ID profile: one parent ID,
ordered References followed by that ID, or one In-Reply-To fallback only when
References is absent. It never invents a Message-ID, recipient or Subject.

`compose_message(to=..., subject=..., body_text=..., cc="", bcc="", reply=None)`
returns MIME bytes. All supplied To/Cc/Bcc choices and Unicode body/Subject remain
explicit; header injection is refused. Validated reply references fold only
between IDs. The host owns parent lookup, chosen recipients/subject, selected
account, write ordering, receipt interpretation and uncertain-write recovery.
A valid MIME message is not evidence that Gmail accepted it into a thread.

Malformed content raises content-free `GmailMessageFormatError` with one of
`invalid_message`, `unsupported_content`, `limit_exceeded`, `invalid_reply`, or
`invalid_composition`, without raw exception cause/context. Returned records and
MIME remain private content; host logging and traceback-local inspection remain
host responsibilities. Intentional stricter behavior replaces the old the paired host
empty/replacement fallback for malformed base64, text and payload structure.

The paired host now uses these helpers directly for full-message projection and
send/draft/reply composition. Its HTML cleanup, including the existing household
sender workaround, remains local. Ordinary valid output remains compatible;
attachment rows additionally expose part identity and incomplete text bodies
carry explicit diagnostics. Raw results, reply outcome handling, account/client
selection, approvals, parent-fetch/send ordering and all stores remain unchanged.

The module derives from the paired host's private `google_mcp.py` helpers. Its donor hash
and exact provenance are in `LICENSE-EXTRACTED`. The extracted Toolkit copy is
licensed under MIT OR Apache-2.0; the original paired host project is unchanged.
Tests are synthetic source/host composition
checks, not live-provider or full end-state acceptance.

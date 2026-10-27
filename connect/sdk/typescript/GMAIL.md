# Gmail operations

Import `GmailOperations` from `@rkat/toolkit/connectors/gmail`. This optional
Node 22+ ESM library runs without Meerkat, MobKit, a browser, Google SDK credentials
or a tool registry. The existing package runtime dependency is `jsonc-parser`
3.3.1; this connector adds no dependency.

```ts
import { GmailOperations } from "@rkat/toolkit/connectors/gmail";
import type { GmailContext } from "@rkat/toolkit/connectors/gmail";

// Existing host connection/auth code selects the account and supplies this binding.
declare const selectedAccount: GmailContext;
const gmail = new GmailOperations(selectedAccount);
const page = await gmail.listMessagesPage({ query: "label:inbox", maxResults: 50 });

// Host approval covers the mailbox, every recipient and the complete MIME bytes.
declare const approvedMime: Uint8Array;
const receipt = await gmail.sendRaw(approvedMime);
```

## Selected host context

`GmailContext` contains `baseUrl`, required `userId`, and
`fetch: (request: Request) => Promise<Response>`. The host supplies an already
selected authenticated Fetch closure. The normal base is
`https://gmail.googleapis.com/`. Explicit HTTP(S) instances and reverse-proxy
prefixes are supported; there is no per-operation URL. The base must be a canonical
absolute URL, with no userinfo, query, fragment, or hidden normalization. For
example, include the slash in `https://gmail.googleapis.com/`. A trailing slash is
added to a nonempty proxy prefix.

`userId` is `me` or a simple ASCII address, at most 320 characters. The supported
address profile is ASCII letters, digits, `.`, `_`, `+`, `-` before `@` and ASCII
letters, digits, `.` and `-` afterward. Unusual syntax is refused, not normalized.
The account route is encoded as one path segment. `me` selects the authenticated
user; an address selects that mailbox path, not a different credential or a grant
of delegated access. Credential/account correspondence remains a host obligation.

The library captures the base, account route and Fetch function once. Later
replacement of context properties cannot retarget that selection. Credential
rotation inside the selected closure, revocation, scopes, admission, recipient
permissions, DNS/proxy/egress policy, TLS, transport pools and transport retirement
remain with their existing host owners. Use separately selected contexts for
separate accounts. Do not change authentication during an operation.

The Fetch closure must preserve the URL, method, body, headers and signal and
return a real Fetch Response. It must not retry, replay, retarget, add default
query parameters or switch accounts. Authentication refresh must not replay a
send. Requests set `redirect: "manual"`, `credentials: "omit"` and `cache:
"no-store"`; authentication is explicit in the host closure. A nonconforming
closure can already disclose credentials before a response URL check. This is a
trusted in-process binding, not a sandbox against hostile JavaScript or Proxies.
Browser transport behavior has not been established by the Node fixtures.

Select REST explicitly. This is not a fallback from denied or unavailable MCP.
Google's [Gmail MCP preview](https://developers.google.com/workspace/gmail/api/guides/configure-mcp-server)
has separate prerequisites and coverage. The host/agent selects the concrete
strategy and supplies its authorized credentials. This package does not discover
accounts, read ambient credentials, acquire tokens or implement OAuth/C1 policy.

## Operations and fidelity

- `listMessagesPage({query, labelIds, pageToken, maxResults, includeSpamTrash,
  signal})` reads exactly one page. It returns the complete object with unknown
  fields, ordering, duplicates, absent fields, opaque continuation and estimate
  unchanged. `maxResults` defaults to 100 and is 1-500. The query and cursor are
  nonempty valid Unicode, at most 8 KiB UTF-8 without control characters. Label
  lists are limited to 100. `gmail.metadata` does not permit a query; the connector
  neither removes a refused query nor broadens scope. The caller owns paging and
  its budget. Repeated cursors are returned, not silently looped or deduplicated.
  An estimate is not a complete-count proof. See [messages.list](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/list).
- `getMessage(messageId, {format, metadataHeaders, signal})` supports `full`
  (default), `metadata`, `minimal` and `raw`. Metadata header selection is allowed
  only for `metadata`, up to 100 names. Nested MIME structure, repeated headers,
  attachment references and unknown fields are retained. No attachments are
  fetched and no mail is decoded or rendered. The returned ID must match the
  selected ID; `raw` format must contain a string raw field. See [messages.get](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/get).
- `sendRaw(mime, {signal})` copies the exact nonempty Uint8Array view synchronously
  before its first await, then base64url-encodes that snapshot in one JSON POST.
  Modifying or transferring the caller's array after entry cannot change the
  sent bytes. SharedArrayBuffer storage is refused because concurrent copying
  cannot establish an immutable snapshot. Detached and empty arrays are refused.
  Byte offsets are preserved; bytes outside the view are not sent. The connector
  does not parse, repair or create MIME, recipients, a Message-ID, thread selection
  or retry key. The host must supply valid RFC 2822 MIME and approve its complete
  To/Cc/Bcc destination set. A provider response is not delivery/read evidence.
  See [sending](https://developers.google.com/workspace/gmail/api/guides/sending)
  and [messages.send](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/send).

Message and label identifiers use ASCII letters, digits, `_` and `-`, at most
1024 characters. Results are mutable `JsonObject`/`GmailMessage` values, not a
projection that discards future fields. Message receipts require nonempty `id`
and `threadId`. Known optional containers receive basic shape checks, not a claim
of full validation of every Gmail resource field.

A Message-ID is not an idempotency key. Searching for it can inform host
reconciliation but an empty result cannot prove an uncertain send never occurred.
There is no automatic resend or exactly-once guarantee. Media/resumable upload,
threaded send, drafts, labels and bulk operations are outside this module.

## Limits, cancellation and uncertainty

Defaults are 20,000ms for the operation deadline, 2 MiB response bytes and 4 MiB
unencoded MIME. Constructor options `timeoutMs`, `maxResponseBytes` and
`maxMessageBytes` change these profile limits. The MIME limit is not a claim about
Gmail's maximum upload size. Byte ceilings bound data consumed by the library,
not eager buffering or decompression performed inside a borrowed transport.

One deadline covers transport and streamed response decoding; chunks do not reset
it. Expiry or caller cancellation aborts the request and cancels an acquired reader,
including a pending read from a borrowed body that does not listen to the request
signal. Each reader is cancelled at most once, and its cleanup is joined before
return. The same Fetch promise remains awaited: a host Fetch that ignores abort
before returning a response, or whose body cleanup never settles, can keep the
operation pending past its deadline. No detached cleanup promise is substituted
for settlement and there is no hard cleanup bound.
Cancellation does not undo remote work. The library does not close the borrowed
host transport.

The library requests identity encoding and requires identity-encoded UTF-8
`application/json`. The shared strict parser rejects duplicate keys (including
escaped spellings), comments, trailing commas, invalid UTF-8, nonfinite numbers,
unsafe integers and more than 128 container levels. Other numeric data uses
JavaScript binary64 precision. This differs from Python's arbitrary-size integers.
Unpaired surrogates in request tokens/queries are refused rather than replaced.

`GmailError` has fixed `kind`, `writeOutcome`, optional `statusCode`, optional
decimal `retryAfterSeconds` and nullable `providerReason`. For HTTP 403 only,
the connector reads at most the smaller of 64 KiB and `maxResponseBytes` to
distinguish documented Gmail failures:

| Provider reason | Kind |
| --- | --- |
| `rateLimitExceeded`, `userRateLimitExceeded` | `rate_limited` |
| `dailyLimitExceeded` | `quota_exceeded` |
| `domainPolicy` | `permission` |

The reported error code must be 403 and its 1-32 error entries must agree on
one documented reason and domain. Malformed, oversized, mixed, unknown or
unreadable diagnostic bodies preserve generic `permission` with no reason.
Cancellation and the total deadline still apply while reading diagnostics.
Other HTTP statuses keep their existing classifications. See Google's
[error reference](https://developers.google.com/workspace/gmail/api/guides/handle-errors).
These observations do not authorize reconnecting, changing quotas or retrying.

No provider message text, URL, MIME, credentials, underlying
cause or caller abort reason appears in the error. Returned provider data itself
can be private. Host hooks, logs, debuggers and in-process access remain host-owned.
Retry-After is information, not permission to repeat a send.

Reads have `writeOutcome: "not_applicable"`. Local input refusals are safe
TypeErrors before Fetch entry. Every failed attempted send has `writeOutcome:
"unknown"`, including HTTP refusal, redirect, malformed receipt, oversize,
timeout, transport loss and cancellation. This conservatively describes missing
effect evidence, not proof that all failures executed. `GmailCancelled` is a
separate typed rejection; JavaScript has no asyncio cancellation hierarchy. An
already-aborted signal sends nothing and uses the same conservative unknown-send
cancellation taxonomy. Other library entry validation still runs before that
signal is observed.

## Provenance and conformance

Original Toolkit code under MIT OR Apache-2.0, based on the linked Google REST
contract and this repository's reviewed Python Gmail/TypeScript HA discipline.
No Google or third-party connector implementation is copied. Google documentation
prose has its stated CC BY 4.0 terms and samples Apache-2.0 terms; Gmail retains its
service terms. The package includes both Toolkit license texts and existing
third-party notices. Cloud project/API enablement, consent/client registration,
scopes and administrator restrictions remain host setup prerequisites.

`tests/gmail.test.mjs` imports the public package subpath and uses real loopback
HTTP sockets, host-authenticated requests and retained fixture cleanup. It covers
page/representation fidelity, same-endpoint accounts, MIME snapshot mutation,
refusal, body limits, post-entry cancellation/loss, progress deadlines, stalled
borrowed readers with delayed or rejecting cleanup, and a host Fetch that ignores
abort. Run it against the exact installed npm tarball as well
as the source build. These are protocol fixtures, not live Google, human consent,
native OAuth/C1, catalog activation or the paired host migration evidence. Complete
M4/M8 and host-migration obligations remain open.

```sh
npm ci --ignore-scripts
npm run typecheck
npm run build
node --test tests/gmail.test.mjs
npm pack --ignore-scripts
```

# Slack conversation reads

`@rkat/toolkit/connectors/slack` supplies `SlackOperations.authTest`,
`listConversationsPage`, `getConversation`, `getUser`, `historyPage` and `repliesPage`.
Each call performs one provider observation request using a host-selected,
authenticated Fetch binding. This is a service connector, independent of a Slack
frontend. It does not admit inbound messages, choose an agent or send replies.

The separate `readInterval` operation below explicitly composes bounded page
reads. The one-request contract remains unchanged for each method listed above.

```ts
import { SlackOperations } from "@rkat/toolkit/connectors/slack";
import type { SlackContext } from "@rkat/toolkit/connectors/slack";

// The application obtains this binding from its existing connection owner.
declare const selectedWorkspace: SlackContext;
const slack = new SlackOperations(selectedWorkspace);
const page = await slack.historyPage("C0123456789", {
  oldest: "1789948800.000000",
  latest: "1790553600.000000",
  inclusive: false,
});
// The caller explicitly chooses subsequent pages or a known thread:
const thread = await slack.repliesPage("C0123456789", "1790000000.123456");
```

The host supplies `baseUrl` (normally `https://slack.com/`) and
`fetch: (request: Request) => Promise<Response>`. Its closure retains the selected
credential; there is no model-supplied account label or token parameter. A second
account needs a separate binding. The library captures the base and function when
constructed. Account verification, scopes, permitted resources, token refresh,
TLS, proxy/DNS policy and transport lifetime remain host responsibilities.

Set `allowedChannelIds` in the constructor options to a readonly array of up to
256 unique exact alphanumeric channel IDs, each at most 128 characters. For
example, `new SlackOperations(selectedWorkspace, { allowedChannelIds: ["C1"] })`
checks channel routing before invoking the host Fetch, including resumed
interval requests. Omission is unrestricted; `[]` denies all channel calls;
`null` is rejected. The constructor privately copies the array. Restricted
instances refuse broad conversation listing; `authTest` remains permitted.
This ceiling does not grant access, bind an account or filter quoted message
content. The host owns those decisions.

Use a canonical HTTP(S) base without userinfo, query or fragment. An explicitly
selected proxy path prefix is preserved. The host Fetch must honor the exact
URL, method, headers, abort signal and manual redirect mode; it must not retry,
retarget, append default query parameters or switch accounts. A response URL
check cannot undo effects already performed by a nonconforming host closure.
Cookies are omitted and caching is disabled. The library never closes the host's
Fetch resources or obtains credentials.

## Explicit identity observation and discovery

`getUser(user, { includeLocale, signal }?)` performs one `users.info` GET for an
explicit alphanumeric user ID, at most 128 characters. `includeLocale` defaults
to false and must be boolean. The returned `user.id` must match exactly; other
fields are preserved, including absent, null and empty profile fields. Names
can change or be shared by multiple users. Keep the ID with any displayed name;
missing or refused lookups remain unresolved rather than guessed.

The host must authorize the optional `users:read` scope and this operation.
`users:read.email` is not needed for names and is not requested automatically.
A channel routing ceiling does not constrain this independently selected
workspace metadata lookup or establish that the user belongs to a channel.
There is no profile cache, implicit lookup during history reads, name resolver
or recipient selection. A second account uses its own binding even for the
same user ID. See [Slack users.info](https://docs.slack.dev/reference/methods/users.info/).

`authTest({ signal }?)` makes one POST with an empty form body. It returns the
provider's authentication observation, requiring bounded alphanumeric `team_id`
and `user_id`. Optional bot/enterprise fields and unknown data remain intact.
This workspace-selected profile refuses missing/null required IDs rather than
inventing an identity; it does not promise all organization-token response forms.
Compare these observations with the host's explicitly selected identity before
separately requested reads. A matching observation is not a grant, audience
policy, durable binding or proof of full readiness. The repository example
`examples/slack-discovery.mjs` demonstrates this comparison and caller-chosen pages.

`listConversationsPage({ types, excludeArchived, cursor, limit, teamId, signal }?)`
performs one GET. Defaults are `types: ["public_channel"]`,
`excludeArchived: false`, `limit: 100`, with cursor/team ID omitted. Types must be
an array of one to four unique `public_channel`, `private_channel`, `mpim` or `im`
values; order is preserved. Limit is an integer 1-999. Flags must be booleans.
Supplied team IDs follow the bounded alphanumeric profile and are exact request
selectors, not proof the provider honored them. Workspace tokens may ignore
`teamId`. Empty nonterminal pages are valid. Row order, duplicates, unknown data,
and absent/null/empty continuation values remain exact. The options and mutable
type array are captured before suspension; no cursor is followed automatically.

`getConversation(channel, { includeLocale, includeNumMembers, signal }?)` performs
one GET for the exact alphanumeric ID. Both flags default false. The returned
channel ID must match; every other field remains intact. Neither method resolves
names or proves that a person is allowed to read the selected conversation.

These additions mirror the applied Python discovery profile and the official
[auth.test](https://docs.slack.dev/reference/methods/auth.test/),
[conversations.list](https://docs.slack.dev/reference/methods/conversations.list/)
and [conversations.info](https://docs.slack.dev/reference/methods/conversations.info/)
contracts checked on 2026-10-02. Provider filtering occurs within cursor pages;
a short or empty result must not be treated as complete retrieval.

Options and result interfaces are exported from both the package root and Slack
subpath: `SlackOperationOptions`, `SlackConversationType`,
`SlackConversationListOptions`, `SlackConversationGetOptions`,
`SlackAuthObservation`, `SlackConversation`, `SlackConversationsPage`, and
`SlackConversationResult`. Unknown fields keep the existing strict `JsonObject`
profile instead of being discarded by the typed envelope.

## History, replies and coverage

History and replies require an explicit alphanumeric conversation ID. `repliesPage`
also requires an existing message timestamp. Timestamps are bounded decimal
strings, copied without numeric conversion or rounding. `oldest`, `latest`,
`inclusive`, `cursor`, `limit` and `includeAllMetadata` select one request; flags
must be actual booleans. The default limit is 15 and the supported input range is
1-999. The provider's actual allowed limit and quota depend on the installation.

These fields follow the official
[history](https://docs.slack.dev/reference/methods/conversations.history/) and
[replies](https://docs.slack.dev/reference/methods/conversations.replies/)
contracts, checked on 2026-10-01. The selected credential must have access to the
conversation and method. A bot credential with history access does not by itself
establish permission to read every kind of thread.

Pages retain message order, duplicates, nested metadata and unknown fields.
`has_more` is preserved when present. Absent, empty and null `next_cursor` remain
distinct. No cursor is followed automatically, and a short page never implies
complete retrieval. See Slack's
[pagination contract](https://docs.slack.dev/apis/web-api/pagination/).

The caller chooses a time interval, conversations, thread coverage and further
pages. Reading history in a window does not establish coverage of replies in
that window to older threads. The library does not resolve natural-language
dates, automatically select conversations, fetch attachments, deduplicate or claim to have read
all workspace updates. Later page failures remain errors.

## Errors, bounds and cancellation

`SlackError` contains a closed `kind`, optional HTTP `statusCode`, an allowlisted
`providerCode` and a bounded numeric `retryAfterSeconds`. Unrecognized provider
codes are omitted. It contains no raw error body, URL, token, cursor, host error,
exception cause or caller abort reason. An HTTP 200 response containing
`ok: false` is still an error. Rate limits never become empty successful pages
or automatic delays/retries. Provider quotas follow Slack's
[current rate-limit policy](https://docs.slack.dev/apis/web-api/rate-limits/).

Only HTTP 200 with uncompressed UTF-8 JSON is accepted. The shared strict JSON
decoder refuses duplicate keys, invalid syntax/encoding, non-finite numbers,
unsafe integers and more than 128 container levels. Returned message timestamps
must remain decimal strings. JavaScript's ordinary binary64 precision applies
to supported non-integer JSON numbers; Python's arbitrary integers are outside
this TypeScript profile. A malformed or oversized response never yields a partial
successful page.

Constructor options default to `timeoutMs: 20_000` and
`maxResponseBytes: 2_097_152`. One deadline covers sending, streamed reading and
decoding. Each call accepts `signal?: AbortSignal`; cancellation produces
`SlackCancelled`, preserving cancellation as a distinct outcome. A call aborted
before transport entry makes no request.

Abort and deadline initiate teardown. The operation still awaits the entered
Fetch and any response reader cancellation before settling, including on error.
No cleanup promise is detached. This requires cooperative host transport code:
the deadline cannot force a nonsettling Fetch or response cancellation to finish.
The byte limit bounds what this library consumes, not the host's internal buffers.

## Explicit bounded interval retrieval

`readInterval(channel, { start, end, rootOldest, maxRequests, maxMessages,
maxResultBytes?, pageLimit?, includeAllMetadata?, resumable?, continuation?, signal? })` explicitly composes the existing
page owners. Defaults are pageLimit 15, includeAllMetadata false and maxResultBytes
262144. Request and message budgets
are positive safe integers. Exact decimal strings require rootOldest <= start <
end; timestamps are never coerced through Number or Date.

The host permits both the output creation window `[start,end)` and the wider
root-discovery interval `[rootOldest,end]`. A sequential invocation-local FIFO
interleaves discovered roots and history continuation with fixed inclusive
requests and opaque cursors. It neither retries nor guesses time-based fallbacks.
reply_count/latest_reply never prove a root has no relevant replies. References
may name roots older than the discovery floor, but roots at or after end are not
queried. Known channel_join/channel_leave skips are counted explicitly.

The portable result uses snake_case keys, including `limits`, `requests_made`,
`messages`, `history`, `threads`, `termination`, `coverage_gaps` and
`skipped_non_threadable`. Each selected occurrence has source, root_ts context
and its exact provider dictionary. Duplicates/conflicting copies remain separate.
No auxiliary historical bodies are added; exact selected broadcast rows may
contain a nested older root rendering. The whole selected page projection must
fit maxMessages and the full-result byte budget before rows, roots or continuation
progress from it are admitted.

Termination is finished/request_budget/message_budget/result_budget/request_failure, with
continuation_budget also available in resumable mode. Finished
means the queue is empty, not complete coverage. Chains have pending/exhausted/
stopped states and closed stop reasons; last accepted cursor/has_more presence,
null and empty values remain distinct. A fresh cursor with has_more=false records
pagination_contradiction and continues. Missing/unusable/cyclic continuation,
invalid linkage, retention or unavailable roots have explicit coverage reasons.
thread_not_found stops one chain while others proceed; other safe SlackError
failures stop globally, preserving earlier observations. Safe errors contain only
kind/status_code/provider_code/retry_after_seconds. SlackCancelled propagates
after existing joined cleanup rather than being converted to partial success.

A positive floor always reports older_roots_unsearched. A zero floor still
cannot prove invisible/deleted roots or an atomic snapshot. Edits/reactions are
not messages created in the interval; permissions, retention and concurrent
changes remain limitations. There is no global complete flag. The configured
max_requests times max_response_bytes bounds admitted response-body bytes, not
raw wire bytes or hard cleanup time. Existing per-page byte/time and JSON numeric
profiles remain unchanged. See `examples/slack-interval.mjs` for explicit host use.

### Full-result byte limit

`maxResultBytes` is an integer from 4096 to 16777216 and is reported as
`limits.max_result_bytes`. It bounds the entire result serialized as compact
ASCII JSON, including escaped Unicode, messages, coverage gaps, chain metadata
and any continuation. It is separate from the per-response byte cap. JavaScript
number serialization remains subject to the existing supported JSON profile.

Page admission reserves room for the next pending page's possible stop or safe
failure result. If a page cannot fit, `termination: "result_budget"` and an
explicit matching chain stop and coverage gap preserve earlier admitted pages.
The refused page contributes no rows, roots, metadata or cursor advancement;
`requests_made` still includes its request. Text is never clipped and there is no
automatic retry. A resumable result retains the exact pre-page step. A later
explicit call may increase only the result budget without changing its selection.
A page containing one oversized message can make no progress at the host's limit.
A smaller page limit requires a new selection rather than modifying its checkpoint.

A supplied checkpoint that cannot fit even the empty-observation stop projection
is refused before HTTP. This result bound does not limit the host transport's
internal buffers or attest complete historical coverage.

### Explicit resumption

Set `resumable: true` to receive `observation_scope: "current_call_only"` and
`continuation`, either a transparent JSON object or null when no work remains.
Omitted or null input starts a new traversal. A nonnull input requires the opt-in
flag. One-shot calls omit the continuation and observation_scope fields.

```ts
const options = {
  start: "1780000000", end: "1780600000", rootOldest: "1779000000",
  maxRequests: 2, maxMessages: 100, resumable: true,
};
const first = await slack.readInterval("C123", options);
// Retain first, including its observations and gaps. The host decides when to resume.
if (first.continuation != null) {
  const next = await slack.readInterval("C123", {
    ...options, continuation: first.continuation,
  });
  // next contains only this invocation's observations, not an accumulated summary.
}
```

The portable object has exactly this shape:

```json
{
  "schema_version": 1,
  "selection": {
    "channel": "C123",
    "start": "1780000000",
    "end": "1780600000",
    "root_oldest": "1779000000",
    "page_limit": 15,
    "include_all_metadata": false
  },
  "pending": [{ "root_ts": null, "cursor": "opaque-provider-cursor", "seen_cursors": ["opaque-provider-cursor"] }],
  "discovered_roots": []
}
```

Selection fields must match exactly, including decimal string representation.
Request, message and result-byte budgets may change between invocations. Each pending root is
unique; null denotes history, and other roots must occur in discovered_roots.
Roots may predate the discovery floor, but must be before end. Seen cursors are
unique within each chain, with the active cursor last; a null cursor requires an
empty list. Unknown keys, invalid types and malformed values are refused before
any request. Plain JSON data is copied before the first await. The object carries
no old rows, page counts, statuses, account identity or grant. The host retains
prior results and resumes with the same selected account and permitted interval.
Caller-supplied state is not evidence that any earlier retrieval occurred.

Bounds are 1024 discovered roots, 1025 pending chains, 2048 aggregate seen cursors
and 262144 compact UTF-8 JSON bytes. The whole page is planned before observations
or checkpoint progress are admitted. If its remaining checkpoint cannot fit,
continuation_budget returns the pre-page checkpoint without that page's rows,
roots, metadata or gaps. Repeating the same request will not remove this bound;
narrow the interval or use the explicit historyPage/repliesPage operations.

Request-budget pauses preserve the queue. A message- or result-budget refusal or ordinary
request failure preserves the current uncommitted step for explicit resumption.
There is no automatic retry, sleep or rate-limit scheduler. A 429 observation
retains its safe retry_after_seconds value for the host to interpret. Cursor
cycles and thread_not_found remain terminal for the affected chain. A later call
does not reconstruct those earlier gaps or prove historical completeness.

Pages and statuses describe only the current call. When a continuation contains
no history step, history is not_requested with zero pages. Threads list only
chains pending or discovered during this call. Cancellation still propagates
after joined cleanup and returns no new checkpoint; replaying an earlier retained
checkpoint can repeat observations. No exactly-once read or provider snapshot is
promised, and expired provider cursors are not replaced with guessed timestamps.

## Distribution and verification

The implementation is original Toolkit code, porting the existing Python Slack
operation contract and reusing the Toolkit Fetch/strict-JSON pattern. No Slack
SDK or other agent framework is included. There are no new runtime dependencies.
Node 22 or later is the supported execution profile. Browser execution has not
been established by the Node test suite.

Local HTTP and controlled Fetch tests exercise pagination fidelity, account
binding, strict response validation, redirects, rate limits, complete-body
deadlines and joined cancellation. These are protocol and package tests, not
live Slack, OAuth, current grants, agent activation or full acquisition proof.
This repository package remains unpublished.

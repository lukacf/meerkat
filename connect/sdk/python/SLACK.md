# Slack read operations

Before sharing a client, the host must call `prepare_httpx_client(client)` once.
See [HTTPX client preparation](HTTPX-CLIENT.md) for the setup and ownership contract.
All client arguments in the examples below are already prepared.

This optional Python operation library provides `SlackOperations.auth_test`,
`list_conversations_page`, `get_conversation`, `get_user`, `history_page` and `replies_page`.
Each call issues exactly one HTTP request
through a borrowed, host-authenticated `httpx.AsyncClient`. It neither owns nor
closes that client. The proposed extra is `meerkat-toolkit[slack]`; this staged
source is local and unpublished.

The host selects the workspace, account, token, granted scopes and client. The
library captures the client's base URL and retains that client reference. Slack
selects the account from its credential, so a workspace label would not verify
account identity. This module does not create such a label or a credential map.
The host must not change the selected credential while using the binding. A
second account requires its own selected client and operation instance.

The host can construct `SlackOperations(client, allowed_channel_ids=["C1"])`
to restrict channel routing. The value is an optional list or tuple of at most
256 unique exact alphanumeric IDs, each at most 128 characters. `None` leaves
routing unrestricted; an empty list denies all channel calls. The constructor
copies the collection, so later mutation cannot widen it. Metadata, permalink,
history, replies and resumed interval calls check the ceiling before call-time
auth or transport. Restricted instances refuse broad conversation listing;
`auth_test` and explicitly selected `get_user` remain permitted. User lookup is
independent workspace metadata access, outside the channel routing ceiling.
This does not grant provider access or filter
quoted content from an allowed channel. The host still owns account binding
and authorization.

Use `https://slack.com/` as the normal base URL, or a deliberately selected proxy
base with a path prefix. The module appends the exact `api/` method path,
preserves that prefix, discards client-default
query parameters and disables redirects per request. Host auth, event hooks,
proxies and transports remain trusted: they must not replay or retarget a
request, leak headers, or suppress cancellation. TLS, refresh, client lifetime
and permission checks remain host responsibilities.

History and replies require an explicit alphanumeric conversation ID, not a channel
name. `replies_page` also requires an existing message's timestamp. `oldest`,
`latest` and `ts` use bounded decimal strings, preserving every character;
floats, exponents and negative values are rejected. `inclusive` and
`include_all_metadata` require actual booleans. The supported page limit is
1-999, defaulting to 15. Cursors are opaque nonempty strings; omit `cursor` for
the first page. There is no time-zone, natural-language or channel resolver.

`auth_test()` makes one POST with an empty form body, using the selected client's
header authentication. Its supported workspace user/bot response requires
nonempty alphanumeric `team_id` and `user_id` (maximum 128 characters each).
Optional bot/enterprise fields and all other provider data remain exact. A
missing/null required ID is an unsupported response; the library never invents
an account. This profile does not claim every organization-token response form.
The observation does not establish a grant, audience, provider readiness for
other methods, or a durable host account binding. The host compares it with its
independently selected identity. See [Slack auth.test](https://docs.slack.dev/reference/methods/auth.test/).

`list_conversations_page(*, types=("public_channel",), exclude_archived=False,
cursor=None, limit=100, team_id=None)` returns one page. `types` accepts a
nonempty list/tuple of unique `public_channel`, `private_channel`, `mpim` or `im`
values in the caller's order. `exclude_archived` is an actual boolean and limit
is an integer 1-999. The optional team ID is a request selector; workspace tokens
can ignore it, so its presence does not verify account selection. Filters and
cursors are snapshotted before suspension. Complete conversation rows and all
unknown fields are preserved. Filters can produce short or empty nonterminal
pages; inspect the actual cursor before explicitly choosing another request.
See [Slack conversations.list](https://docs.slack.dev/reference/methods/conversations.list/).

`get_conversation(channel, *, include_locale=False, include_num_members=False)`
requires a returned conversation object with that exact ID and preserves all
other fields. Both flags require actual booleans. Provider metadata is not a
current audience or membership attestation. See
[Slack conversations.info](https://docs.slack.dev/reference/methods/conversations.info/).

`get_user(user, *, include_locale=False)` makes one `GET api/users.info` for an
explicit alphanumeric user ID (1-128 characters). `include_locale` must be a
boolean. A successful response must contain a user object with the exact
requested ID. All other fields, including absent, empty, null and unknown
profile data, remain unchanged. `user_not_found` produces safe `not_found`
metadata. There is no cache, batch lookup or automatic message enrichment.

Select this operation separately when author or mention names are needed. The
host must authorize `users:read`; the connector does not request or widen scopes.
Names do not require `users:read.email`. A returned profile does not establish
channel membership, audience access or account identity. See
[Slack users.info](https://docs.slack.dev/reference/methods/users.info/).

[The public usage example](examples/slack_discovery.py) compares provider identity
with explicit host-selected IDs, then shows caller-chosen pages, detail, history
and replies. It has no executable credential setup or automatic traversal.

These request fields follow Slack's [history reference](https://docs.slack.dev/reference/methods/conversations.history/)
and [replies reference](https://docs.slack.dev/reference/methods/conversations.replies/).
Access depends on token type, conversation membership and the applicable
`channels:history`, `groups:history`, `im:history` or `mpim:history` permission.
The host must validate the selected installation's actual access.

Successful history/reply pages preserve message order, duplicates, nested data and optional
fields. Message timestamps must remain strings. The module keeps absent,
empty and null `next_cursor` values distinct and preserves `has_more` when
present. A short page does not imply completion. These one-page methods do not follow cursors,
sort, deduplicate, fetch attachments or discover threads. Slack's
[pagination contract](https://docs.slack.dev/apis/web-api/pagination/)
describes cursor continuation and expiry. A history window alone does not
establish that replies during that window to older threads were fetched.
Access restrictions, retention, pagination and concurrent changes also limit
coverage. No result here means "all Slack updates for the week."

## Explicit bounded interval retrieval

`read_interval(channel, *, start, end, root_oldest, max_requests, max_messages,
page_limit=15, include_all_metadata=False, max_result_bytes=262144)` is an opt-in collector over those
unchanged page owners. The host must permit both the output creation window
`[start,end)` and the wider root-discovery interval `[root_oldest,end]`. Timestamp
strings are compared exactly as decimals, without floats or normalization.
Request/message budgets must be positive safe integers; root_oldest must be
<= start < end. The complete-result byte budget must be an integer from 4096
through 16777216, inclusive.

One sequential FIFO interleaves discovered threads with history continuation.
Every request uses fixed inclusive bounds and opaque cursors; there is no time
fallback or retry. Roots are considered without trusting reply_count/latest_reply
as skip evidence. A valid reference may point below the discovery floor. Roots
at or after end cannot have an in-window child and are not queried. Known
channel_join/channel_leave occurrences are counted as non-threadable skips.

The result has exact selected occurrences in `messages`, each with source and
root context, separate `history`/`threads` chain outcomes, `coverage_gaps`, and
configured `limits`. Duplicate and conflicting rows remain separate. No raw
historical page dump or auxiliary older body is returned; exact selected
broadcasts can contain their provider-supplied nested root rendering. Whole-page
selected projections must fit max_messages before any rows, roots or progress
from that page are admitted. A refused page leaves prior cursor evidence intact.

`termination` is finished, request_budget, message_budget, request_failure,
continuation_budget or result_budget.
Finished only means no queued work. Chains distinguish pending/exhausted/stopped
and preserve absent/null/empty cursors from their last admitted page. Contradictory
has_more=false with a fresh cursor records a gap and follows the cursor. Missing,
unusable or cyclic continuation stops that chain with a reason. Invalid thread
linkage, retention markers and unavailable threads are explicit gaps.
thread_not_found permits other queued chains; other safe SlackError failures
stop globally with only safe metadata. Cancellation propagates after the existing
request owner joins cleanup; it never becomes partial success.

A positive floor always reports older_roots_unsearched. Even a zero floor and
exhausted visible chains cannot prove an immutable snapshot: edits/reactions are
not creation-time messages, and permissions, retention, hidden/deleted roots and
concurrent changes can limit observations. There is no global complete flag.
max_requests times the existing per-page max_response_bytes bounds admitted
response bodies, not raw wire bytes or hard cleanup time. Per-page timeouts and
client/account ownership are unchanged. See [the explicit host example](examples/slack_interval.py).

`max_result_bytes` separately limits the complete result encoded as compact JSON
with ASCII-escaped strings and UTF-8 bytes. It includes `limits.max_result_bytes`,
all messages, metadata, errors, coverage gaps and any continuation. Unicode,
quotes, backslashes and control-character escape expansion count. This is a
result-data bound, not a process-memory or transport-envelope limit. A transport
that repeats or wraps the result must account for its own additional bytes.

A page is admitted only when its whole projection fits and leaves room for the
next pending step's bounded stop/error return. A `result_budget` stop retains all
earlier admitted pages and gaps, marks the current chain stopped, and does not
advance that page's cursor or admit its rows, roots or coverage metadata. Its
request still counts in `requests_made`. Even a single first-page message can be
too large: the result then contains no admitted messages, an explicit gap and,
when resumable, the unchanged pending step. There is no clipping or automatic
retry. Never interpret this stop as an empty or complete interval.

Increase an allowed byte budget on an explicitly authorized later call, or start
a new read with a narrower query or smaller page limit. A single message may
remain unavailable at the maximum permitted budget; retain that limitation in
any summary. An incoming checkpoint whose initial state and stop/error return
cannot fit the selected byte budget is refused before a provider request. An
unusually oversized error produced by a custom host transport is also withheld
as `result_budget`, with the attempted step retained rather than an oversized
result being returned.

## Resume an interval read

Pass `resumable=True` to opt into a bounded caller-held continuation. Return the
result's nonnull `continuation` unchanged on a later call with `resumable=True`
and the same channel, start/end/root_oldest, page_limit and metadata option.
Per-call request/message/result-byte budgets may change. No automatic delay or retry occurs;
the caller decides when to continue, respecting a returned Retry-After.

Each result has `observation_scope: "current_call_only"`: messages, page counts,
errors and coverage gaps describe that invocation. A history chain absent from
the pending work has state `not_requested`, not `exhausted`. Keep all earlier
parts and their gaps. A null continuation means no queued work remains, not
that all workspace updates were observed. One-shot calls also apply the complete
result cap, without adding a continuation unless requested.

The version-1 object contains exact selection fields, ordered pending
`{root_ts, cursor, seen_cursors}` entries and `discovered_roots`. It carries no
messages, credentials, cumulative result, prior-status assertion or account
proof. Validation copies it before requests and rejects unknown fields, changed
selection, invalid roots/cursors and structural/UTF-8 size limits. Limits are
1024 roots, 1025 pending steps, 2048 total seen cursors and 262144 compact JSON
UTF-8 bytes. Preserve it with the same host-selected account/client; this plain
query data is not authenticated and cannot enforce that account association.

Only complete admitted pages advance the checkpoint. A request failure retains
the failing step, a message/result-budget refusal retains the uncommitted page, and a
request-budget stop retains the untouched queue. If a page would exceed the
checkpoint capacity, `continuation_budget` returns the valid prepage checkpoint
without any of that page's rows, roots or coverage metadata. A larger request
budget will not fix that capacity limit; start a narrower query or use the raw
page operations. Cursor cycles and unavailable threads remain explicit gaps.

Replaying or forking a checkpoint can repeat reads. Cancellation propagates after
request cleanup and provides no new acknowledged checkpoint; a lost response may
require refetch. The connector supplies no durable storage or exactly-once read
promise. Provider cursors may expire and failures never trigger hidden fallback.

```python
selection = dict(start="10", end="20", root_oldest="0",
                 max_requests=2, max_messages=100, resumable=True)
part = await slack.read_interval("C1", **selection)
# Retain this part and its gaps. Later, after the caller authorizes another read:
if part["continuation"] is not None:
    next_part = await slack.read_interval(
        "C1", **selection, continuation=part["continuation"],
    )
```

## Read one explicit page

```python
from meerkat_toolkit.connectors.slack import SlackOperations

# selected_slack_client is supplied and later closed by the host.
slack = SlackOperations(selected_slack_client)
page = await slack.history_page(
    "C0123456789",
    oldest="1789948800.000000",
    latest="1790553600.000000",
    inclusive=False,
)
# This is one page of one explicitly selected conversation and interval.
# Pass a returned cursor only after the caller chooses another request.
# Read a known thread separately with replies_page(channel, exact_message_ts).
```

`SlackError` reports a failure kind, HTTP status, an allowlisted provider error
code when recognized, and a bounded numeric `retry_after_seconds` if supplied.
It does not expose raw error bodies or unknown error strings. `ok: false`
always fails even under HTTP 200. HTTP 429 is a rate-limit error, never an empty
successful page. No failure triggers a retry or a delay. Slack applies rate
limits by app, workspace and method; installation categories differ. The
15-item default is a conservative request size, not proof of a usable quota.
See Slack's [current rate-limit rules](https://docs.slack.dev/apis/web-api/rate-limits/).

Only HTTP 200 with an uncompressed JSON response is supported. Duplicate JSON
keys, non-finite numbers, malformed required fields and bodies over the byte
limit fail. One deadline covers sending and reading the complete stream.
Cancellation stays cancellation. One retained operation task owns the complete
HTTPX request and unwind, and is joined before returning or propagating caller
cancellation. Caller cancellation and the deadline use the same public AnyIO
cancel scope; repeated raw caller cancellation cannot interrupt an entered
transport close. The explicit final close is shielded too. The deadline
initiates cancellation, while joining cooperative cleanup can extend beyond it.
There is no detached cleanup, client closure, retry, or unjoined timeout escape.
Finite completion still requires cooperative host hooks, auth and transport
cleanup; arbitrary code that suppresses cancellation or stalls close has no
hard completion guarantee. The optional extra declares AnyIO directly.
The host owns logging and must keep URLs, cursors, credentials and message
content out of inappropriate logs or audiences.

This is original Toolkit code under the SDK's MIT OR Apache-2.0 licensing. No
Slack SDK code was copied. Real loopback tests are protocol evidence, not a
live Slack account, OAuth, tool activation, agent continuation or complete
capability-acquisition receipt.

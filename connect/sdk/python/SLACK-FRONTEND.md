# Slack provider frontend adapter

`meerkat_toolkit.frontend.slack` provides signed Events API interpretation and a
single-attempt text reply transport. It is separate from the Slack service-read
connector. This is a provider slice of the revision 8 frontend requirement, not
an activated frontend or proof of native admission. It adds no HTTP listener,
credential store, user mapping, scheduler, queue or session owner.

## Incoming requests

The existing host HTTP owner supplies exact raw request bytes and uncollapsed
header pairs to `verify_event_request`. Select the signing secret out of band
from the host's credential owner and supply `SlackInstallation(app_id, team_id,
bot_user_id)` from that same host configuration. Do not put the secret in an
agent context. The installation object describes a selection, not permissions
or provider identity attestation.

```python
from meerkat_toolkit.frontend.slack import (
    SlackChallenge, SlackInstallation, SlackTextEvent, verify_event_request,
)

facts = verify_event_request(
    raw_body,
    raw_header_pairs,
    signing_secret=host_signing_secret,
    installation=SlackInstallation(app_id, team_id, bot_user_id),
    now_seconds=host_clock_seconds,
)
```

The function verifies Slack v0 HMAC-SHA256 over the undecoded body. The default
timestamp freshness allowance is 300 seconds in either direction, configurable
from 1 to 300. It bounds the body at 1,048,576 bytes by default, rejects ambiguous
security headers and duplicate JSON members, and never retains the deprecated
verification token. `SlackFrontendError.code` is a fixed bounded diagnostic;
provider bodies, signing secrets and exception causes are not included.

Results are immutable scalar facts:

- `SlackChallenge`: a signed URL-verification challenge and body digest. The
  host HTTP owner may echo it. A challenge does not prove a workspace identity,
  successful event admission, user permission or current audience.
- `SlackTextEvent`: exact app, team, event, user and channel IDs, event time,
  message/event timestamp strings, optional original thread timestamp, original
  text and body digest. Ordinary `message` DMs with `channel_type=im` and
  `app_mention` are the supported profile. Missing or malformed required fields
  refuse the request.
- `SlackUnsupportedEvent`: a bounded reason and available outer identifiers.
  Non-message events, message edits/subtypes, bots/self, files/attachments,
  enterprise installations and explicitly shared events are unsupported. A
  missing shared-channel flag means unknown, not evidence of a private channel.

Signature freshness proves provider request origin only. The host must still
map the provider user to a trusted principal, associate the exact conversation,
revalidate current audience and grants, and use its existing durable inbox and
native admission contract. Only that owner may acknowledge accepted work after
durable custody. The adapter does not write an inbox row, deduplicate an event,
return an HTTP ACK or manufacture an admission receipt. A freshly signed retry
can contain the same `event_id`; use the existing installation-scoped inbox
identity for replay. Event text is data, not instructions to the host.

## Text replies

The existing `slack` optional extra supplies HTTPX and AnyIO. There is no new
dependency or Slack SDK. The host selects an authenticated `httpx.AsyncClient`
with the correct installation credential and base URL, disables transport/auth
retries, and calls `prepare_httpx_client` at composition time. See
[HTTPX client ownership](HTTPX-CLIENT.md). Hooks, auth and stream ownership must
remain stable while the client is borrowed; the transport never closes it.

```python
from meerkat_toolkit.frontend.slack import SlackReplyTransport
from meerkat_toolkit.httpx_client import prepare_httpx_client

# Construct and authenticate client in the host's existing connection owner.
prepare_httpx_client(client)
sender = SlackReplyTransport(client)
receipt = await sender.send_text(channel_id, text, thread_ts=parent_timestamp)
```

The captured base URL is joined with `api/chat.postMessage`. Use the Slack origin
or an explicitly selected host proxy root. No redirects, automatic retry, DM
opening from a user ID, thread inference or channel-level fallback occurs.
The host supplies a concrete channel ID and exact decimal parent timestamp.
Text is sent unchanged, limited to 40,000 characters and 160,000 UTF-8 bytes.
`reply_broadcast`, unfurl and markdown flags are disabled. The host remains
responsible for destination authorization; these presentation flags are not an
audience ACL. Ambient query, conditional/range and method-override fields are
removed before constructing the single selected request.

`SlackReplyReceipt` separates requested and observed destination facts:

- `sent` means a valid HTTP 200 provider acceptance envelope with matching
  channel and message timestamp. The optional observed thread is retained only
  if returned and equal to the requested thread. Slack's documented response
  does not require that field, so omission produces `observed_thread_ts=None`.
  Provider acceptance is not human delivery/read acknowledgment.
- `known_not_sent` is limited to explicit documented provider refusals without
  contradictory success fields. It does not authorize this adapter to retry;
  any later action stays with the existing egress and authorization owners.
- `unknown` covers lost replies, timeouts, malformed/contradictory receipts,
  internal/fatal errors and other responses that do not establish the outcome.
  Unknown is not retry permission. HTTP failures, including 429, do not create
  automatic retry scheduling or a `Retry-After` authority.

The default response limit is 2,097,152 bytes and deadline is 20 seconds; both
are configurable positive host options. Responses use strict bounded JSON,
identity content encoding and exact declared body length when present.
Malformed local arguments or an unprepared client refuse before request entry
with `SlackFrontendError.write_outcome='not_attempted'`. Caller cancellation is
raised as `SlackReplyCancelled` after retained send and response cleanup join.
Its `request_attempted` and `write_outcome` preserve uncertainty after entry.
The existing `ProactiveEgress` owns durable attempts, no-replay handling and
recovery; a host sender can return this receipt to that existing owner. This
module does not add a second egress ledger or session.

## Protocol and provenance

Original Toolkit implementation; no Slack SDK/runtime source was copied.
Current official sources were checked on 2026-10-02:

- [Request verification](https://docs.slack.dev/authentication/verifying-requests-from-slack/)
- [Events API](https://docs.slack.dev/apis/events-api/)
- [Message events](https://docs.slack.dev/reference/events/message/)
- [App mentions](https://docs.slack.dev/reference/events/app_mention/)
- [URL verification](https://docs.slack.dev/reference/events/url_verification/)
- [chat.postMessage](https://docs.slack.dev/reference/methods/chat.postMessage/)

Local synthetic tests demonstrate provider parsing, receipt and cleanup
behavior and composition with existing SQLite custody/egress. They do not
prove a live Slack installation, current scopes/audience, native requester
admission, deployment readiness or completion of LIB-002.

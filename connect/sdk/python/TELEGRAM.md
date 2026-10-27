# Telegram frontend transport and provider adapter

`meerkat_toolkit.frontend.telegram` supplies the reusable Telegram Bot API wire
transport and its send receipt classifier. It uses the Python standard library
and adds no SDK dependency. Importing or constructing it performs no I/O.
The optional `meerkat-toolkit[telegram]` extra adds the existing pinned
`python-telegram-bot==22.7` parser used by
`meerkat_toolkit.frontend.telegram_adapter`. Importing that adapter does not load
the parser; normalization loads it only when called.

The host supplies one already-selected token to `TelegramBotApiTransport` and
owns recipient selection, identity, admission, authorization, durable inbox and
outbox rows, polling cursor commits, media authorization, webhook authentication,
retry policy and lifecycle activation. The transport neither discovers accounts
nor installs credentials. `TelegramTransport` remains injectable for hosts with
a different network implementation; an injected transport must uphold the same
receipt and cancellation contract. Such an injection is trusted host code, not
an agent-selected origin override.

The supported operations are `get_me`, `get_webhook_info`, `get_updates`, `set_webhook`, `delete_webhook`,
`get_file`, `download_file`, `send_message`, `send_photo`, `send_chat_action` and
`close`. Uploads contain explicit bytes, not provider-fetched media URLs.
`download_file` enforces the host's byte ceiling while reading. API JSON responses
are limited to 64 MiB and reject duplicate members and non-finite JSON values.
Text and caption entities preserve the original plain wire shape when absent;
this module never sets `parse_mode`.

## Inspect before frontend activation

The concrete `TelegramBotApiTransport` supplies these inspection methods; the
existing injectable `TelegramTransport` interface remains unchanged.
`get_me()` reads the selected credential's bot account and returns
`TelegramBotIdentity`: provider bot ID, optional username, and optional group
capability flags. `get_webhook_info()` returns `TelegramWebhookInfo`: whether a
webhook is configured, its certificate flag, pending update count and optional
allowed update types. Absent optional values remain distinct from explicit false
or an empty list. The callback URL and raw webhook diagnostics are never returned.
These are read-only [getMe](https://core.telegram.org/bots/api#getme) and
[getWebhookInfo](https://core.telegram.org/bots/api#getwebhookinfo) observations.

For a webhook host, `get_webhook_info(expected_url=host_callback_url)` compares
the provider URL with that host-supplied nonempty string exactly, without
normalization. `matches_expected_url` is `None` when no comparison was requested,
`False` when no webhook is configured or another URL is configured, and `True`
only for an exact match. `configured` distinguishes absence from another URL.
Neither URL is returned or sent as a request parameter; HTTP authentication and
response validation still precede comparison. Empty or non-string expectations
raise a generic `ValueError` before transport entry. Existing no-argument callers
and the injected transport protocol are unchanged.

`has_last_error` reports whether Telegram supplied `last_error_date` or
`last_error_message`, including an explicit zero timestamp or empty message.
`last_error_date` preserves its validated Unix timestamp or `None` when absent;
the message text is never returned. These describe the last reported webhook
delivery error, which may be historical. Absence is not proof of current health,
and an old error does not automatically block activation. Datacenter
synchronization errors and other unprojected fields are outside this observation.
Malformed projected fields, including explicit nulls, refuse the whole result.
The timestamp profile is a nonnegative signed 32-bit integer, following the
[WebhookInfo](https://core.telegram.org/bots/api#webhookinfo) and
[Integer type](https://core.telegram.org/bots/api#available-types) contracts.

The host can use them before activation to verify the intended bot account and
notice an existing inbound owner. Neither call changes a webhook, polls messages
or sends anything. No configured webhook does not establish that no other poller
is active. The host still authorizes any change and owns admission, routing,
recipient policy and lifecycle. These observations do not prove an admitted
conversation, successful reply or installed frontend readiness.

The existing paired host Telegram adapter's `activate()` is the consumer seam:
inspect using its selected credential before `set_webhook` or marking polling
active, and let the existing host decide whether the selected binding may proceed.
That host integration is separate work; adding these transport methods does not
change an installed the paired host adapter or activate a frontend.

## Delivery observations

Use `classify_send_response` only for direct authenticated `sendMessage` and
`sendPhoto` responses, not chat actions or responses embedded in webhook replies.
It returns `TelegramSendReceipt`:

- `provider_accepted`: HTTP 200, literal `ok: true`, an object result and a positive
  integer message identifier, with no contradictory failure fields. This is
  provider acceptance, not proof of recipient reading.
- `provider_queued`: the same success envelope with message identifier zero,
  which is not a usable sent-message receipt. Do not resend it automatically.
- `known_not_sent`: a complete explicit refusal, including literal `ok: false`,
  a nonempty string description and an integer error code matching HTTP
  400, 401, 403, 404 or 429. No result is allowed. Other/new codes remain unknown.
- `unknown`: missing, malformed or contradictory envelopes, redirects, server
  errors and all other unrecognized send observations. A transport exception
  after entry likewise never proves non-delivery.

The presenter binds both text and photo results to the requested destination.
Each invocation performs at most one provider send. A numeric recipient
requires the same `Message.chat.id`. An `@username` recipient requires a valid
chat ID and an exact, case-sensitive match to `Message.chat.username`; the
adapter does not resolve aliases. An explicitly selected topic requires the
same returned `message_thread_id`. Without a selected topic, an absent or valid
provider-assigned topic is allowed. Missing, malformed or contradictory bound
facts produce `unknown`, including for a queued message, and never trigger
another send. This checks the provider's observation, not actual delivery or
the host's recipient authorization.

Direct callers can use `classify_send_response(response,
expected_recipient=..., expected_message_thread_id=...)` for these checks.
The legacy call with neither expected field remains an envelope-only check.

`retry_after_seconds` preserves validated provider metadata; it does not grant
permission to send. The transport never retries or changes the recipient.
Descriptions are bounded and controls removed; token text is redacted from
provider error descriptions. Network exception messages and ordinary tracebacks
omit credential-bearing URLs.

The [official Bot API contract](https://core.telegram.org/bots/api#making-requests)
defines success and error envelopes. The
[Message contract](https://core.telegram.org/bots/api#message) documents the zero
identifier case. The explicit refusal subset and conservative treatment of
server failures are this adapter's policy, not a Telegram exactly-once promise.
Pinned primary-source snapshots and the reviewed server source revision are in
the extraction evidence directory. No Telegram server source was copied into
the Python implementation.

## Transport custody and limits

The default client uses fixed `https://api.telegram.org` API and media origins,
normal TLS verification, no redirects and no ambient proxy configuration. A
credential-bearing request is not forwarded to a redirect target. It does not
accept a user-controlled endpoint. Each instance belongs to one asyncio loop.

Each entered blocking operation has one retained task. Caller cancellation,
including repeated cancellation, joins that task before propagating the first
cancellation; a late transport failure does not replace it. `close` rejects new
operations and joins the current ones, also through cancellation. It does not
close another instance, rotate a credential, delete a webhook or log the bot out.
Hosts must call `close` during orderly shutdown before stopping the event loop.
Cancellation before operation entry performs no request.

The 30-second timeout is a socket-operation timeout, not a total wall-clock
bound. A slow stream or uncooperative thread can delay cancellation and close.
There is no claim of hard thread preemption or custody after forced process loss.
A durable owner must record entry before sending and preserve uncertainty after
process loss. The existing Toolkit `ProactiveEgress` is that owner in the
The paired host adoption, with no additional queue or retry machine here.

## The paired host adoption and provenance

The isolated `paired-host-ext-telegram` adoption retains its frozen API-v1 types,
inbound identity mapping, channel lifecycle, provider token lookup, typing
behavior, host limits and authorized media retrieval. Its `provider.py` is a
compatibility import. Provider interpretation and presentation now live in the
portable adapter described below. Result projection and eligibility for a
plain-formatting retry consume the same classifier, so a malformed 400 cannot
make a second send eligible. Current source delegates that second attempt to
the durable owner; adopting hosts must forward the new receipt hint described
below. Existing built the paired host distributions retain their recorded version.

API-v1 has no queued outcome. It conservatively maps `provider_queued` to
`unknown`, preserving the fixed scheduled/unusable-receipt explanation. It must
never map that observation to `sent`, `known_not_sent`, or a new automatic send.
This is an explicit compatibility limitation, not full queued-state projection.

The extracted starting file is `src/paired_host_ext_telegram/provider.py` at
The paired host Telegram commit `f187bf89f49d4a0444fd3a9bffe07a49ab662f51`.
The captured user-owned project declared its license as `Private` and had no
root LICENSE/COPYING/NOTICE file. Its source provenance and notices remain
preserved. The extracted Toolkit copies are licensed under MIT OR Apache-2.0
as stated in `LICENSE-EXTRACTED`; this grant does not change the original
The paired host project's license or separately distributed dependencies.
The optional `python-telegram-bot==22.7` dependency (LGPL-3.0-only) supplies update
models through Toolkit's `telegram` extra. The extension selects that extra;
the version is unchanged and no PTB source is copied. The new provider adapter
is also PairedHost-derived and covered by `LICENSE-EXTRACTED`. Exact files,
metadata, dependencies and hashes for the original wire extraction are in
`docs/coordination/20260930-upstream-review/telegram-frontend-transport-extraction`.
The adapter successor is recorded in
`docs/coordination/20260930-upstream-review/telegram-frontend-adapter-successor`.

## Portable interpretation and presentation

`normalize_update(payload)` returns a `TelegramEvent` with the provider update
identifier, a normalized envelope and `TelegramMediaRef` values. Thread, reply,
sender, chat and media references retain the existing donor wire meaning. These
are provider facts, not trusted principals, authorized media, or admission.
The host maps them into its own API and connection scope. Toolkit imports no
The paired host API types.

The conversational profile is human-authored text, photo, document and voice.
Other content returns `TelegramUnsupportedEvent(update_id, reason, facts)`.
Its reasons distinguish service messages, unsupported content, unknown fields
and sender contexts that the current envelope cannot represent. `facts` retains
the normalized identity and accompanying text/caption. It contains no raw
unsupported attachment body and deliberately exposes no `envelope` or `media`
attribute: callers must handle this outcome explicitly before admission.
Empty text is unsupported; an empty caption with valid media is preserved.
The parser does not turn service data, bot-authored messages, anonymous-chat
senders or business conversations into human instructions.

The development adapter also interprets `edited_message` as an unsupported
outcome, never as a replacement agent turn. Recognized human edits use reason
`message_edit`; service-message, unknown-field and sender-context refusals retain
their own reasons.
Edit facts retain routing identities, chat type and sender-kind observations,
but omit the changed text, caption and downloadable media. A host may use these
facts under its own policy to authorize a fixed notice. The parser grants no
permission to send one.

Unknown fields remain explicit even when the pinned PTB parser does not know
them. Known contradictory content and malformed shapes refuse normalization.
Telegram's documented animation/document, venue/location and live-photo/photo
aliases produce one unsupported outcome. They never become a partial document,
location or photo. These classifications follow the
[Bot API message contract](https://core.telegram.org/bots/api#message) and the
[pinned PTB parser](https://docs.python-telegram-bot.org/en/v22.7/telegram.message.html).

`interpret_poll_page(updates, cursor)` interprets one complete response and
returns `TelegramPage(events, next_cursor, ignored_updates)`. The cursor is a
candidate only. The host commits it after durable admission under its existing
inbox owner. `events` retains supported and unsupported outcomes in provider
order; the host must durably admit or reject every outcome before committing
the cursor. A valid unsupported message does not block later supported rows.
Fallback notices, if offered, use the host's authorized delivery owner; the
parser does not send anything. Valid update IDs for update types with neither
`message` nor `edited_message` remain deliberately ignored. A present malformed
message or edit, including null or a non-object value, refuses the whole page
before any events are returned. This corrects the donor's silent omission of
those malformed rows;
no earlier good row or candidate cursor may escape that refused page.

`validate_media_request` checks the local reference and positive byte bound
before credential resolution. `fetch_media(transport, ref, max_bytes)` uses a
borrowed transport for a reference the host has already authorized. It returns
`TelegramFetchedMedia` and does not close the transport, choose the byte limit,
authorize a MIME type or store the content. `TelegramAdapterError` carries a
bounded `invalid_request`, `provider_error` or `runtime_not_ready`
classification; retryability is diagnostic and grants no retry authority. A
missing optional parser is `runtime_not_ready`, not a malformed user update;
the composing host supplies the selected extra, with no automatic installation.
Default parser and media failures do not retain raw provider exception text,
cause or context. A composing host may select its already-sanitized declared
error classes with `fetch_media(..., passthrough_errors=(HostError,))`; only those
explicitly selected classes propagate unchanged. This is a host contract, not a
model argument or a rule inferred from exception text. The paired host shim selects
only its existing `ExtensionCallError`. Invalid UTF-8 media references and
filenames are refused by the adapter's bounded validation. The paired host shim
also refuses invalid UTF-8 outbound text and captions before secret access or
provider entry. The portable presenter does not impose that host text policy.

`TelegramPresenter.send_text` and `send_photo` accept a borrowed transport and
host-authorized recipient/content. They preserve the donor's closed formatting
entity profile and UTF-16 offsets. Invalid local formatting is stripped before
the single provider call. The presenter reports
`entity_strip_fallbacks` and `entity_wire_rejections`; it owns no delivery store
or lifecycle. A complete explicit 400 refusal of valid formatting returns
`known_not_sent` and `retry_without_formatting=True` on `TelegramSendReceipt`.
The presenter does not send again. All other observations carry the default
false hint. Queued, unknown and interrupted delivery never make a formatting
retry eligible. Host typing and send lifetime remain with the extension.

When composing with `ProactiveEgress`, the host sender forwards this boolean
and maps `provider_accepted` to `sent`, `provider_queued` to `unknown`, and the
other outcomes unchanged. The [existing delivery owner](FRONTEND.md#outcomes-and-retry-evidence)
atomically retains the refusal and clears optional formatting. It then checks
the current sender, permission, freshness and attempt budget before another
call. Hosts without that owner must provide equivalent durable per-attempt
control; they must not recreate an unchecked retry inside the sender wrapper.
Dropping the hint preserves the refusal but does not request a plain candidate.

UNKNOWN retries are a separate host decision. The shared owner's default
`EgressDeliveryPolicy()` does not resend uncertainty. A host can select the
[bounded persisted policy](FRONTEND.md#select-a-bounded-uncertainty-policy), with
every new provider call separately authorized and reserved; the paired host's accepted
choice is one UNKNOWN resend within four total attempts for conversational
Telegram replies only, never consent- or approval-governed effects. The presenter does not
implement that retry or treat a queued response as acceptance. Durable Stop,
uncertainty history, startup quiescence and state compatibility stay with the
existing owner and its host composition.

For an explicitly host-authorized topic, pass `message_thread_id` to
`TelegramPresenter.send_text` or `send_photo`. The same optional keyword is
available on `TelegramBotApiTransport.send_message` and `send_photo` and in the
`TelegramTransport` protocol. The ID is sent as an integer in text JSON and as a
decimal field in photo multipart data. Both local entity stripping and the
owner-managed plain-formatting retry preserve the selected topic. The host
must retain and authorize that topic association on each invocation.

The bounded profile accepts `None` or an exact Python `int` from 1 through
2147483647. Booleans, strings, floats and other values raise `ValueError` before
transport entry; they do not become an unknown-send receipt. `None` omits the
provider field and the presenter's borrowed-transport keyword, preserving
ordinary-chat call behavior. A custom transport must support the optional
keyword to send to an explicitly selected topic; Toolkit does not retry by
dropping it. The host must select and authorize the recipient/topic pair. An
incoming event's `thread_id` is only a provider fact and is never automatically
promoted into an outgoing destination.

The official Bot API documents this optional field for
[sendMessage](https://core.telegram.org/bots/api#sendmessage) and
[sendPhoto](https://core.telegram.org/bots/api#sendphoto) in forum supergroups and
private chats of bots with topic mode enabled. The
[Integer type contract](https://core.telegram.org/bots/api#available-types) uses
signed 32-bit values unless otherwise noted; positive-only IDs are this SDK's
bounded profile. This adds no topic creation, permission check, direct-messages
topic support, or topic-scoped `send_chat_action`. Typing remains the host's
separate concern.

Local loopback/process tests establish this composition's behavior. They do not
establish live Telegram readiness, account enrollment, platform C1 admission,
publication, deployment, or completion of the full frontend requirements.

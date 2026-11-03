# Live (realtime) member sessions through the gateway

Status (audited checkout): MobKit 0.8.39 uses exact crates.io Meerkat 0.8.40
dependencies, as recorded in the manifests and `Cargo.lock`. The console Live
owner seams are available from those registry dependencies; no Git development
patch is required by this checkout. The 0.8.38 gap survey and September
development-pin lineage below are historical, not current rebinding instructions.
Unconfigured gateways remain unavailable. Real-provider/browser audio operation
must be distinguished from deterministic transport and shared-owner tests.
The registry repin alone does not establish new real-provider/browser acceptance.
The backend fixture crosses authenticated HTTP, a loopback provider HTTP/
WebSocket boundary, explicit SDP-delivery acknowledgement, generated activation,
active-target readiness, and close. Only the provider is simulated; runtime
receipts are issued by the shared Meerkat owners.

Consumer shape: a LAN client (the downstream app's device client or a satellite process) opens
a realtime audio/text channel to a Mob member identity. The GPT Live endpoint
owns conversation and transport only. When GPT Live initiates client
delegation, Meerkat admits the exact canonical final transcript. The default
stdin integration uses an ordinary durable executor fork; console voice opts
into the existing selected member. Tools and callbacks belong to the ordinary
executor, never to the voice endpoint. The canonical conversation
persists through Meerkat's normal transcript authority.

## Console voice: current integration boundary

Do not infer console voice support from the public GPT Live model registration.
The currently released strict profile is
`openai.gpt-live-1.client-context.v1`; it is not the legacy `gpt-realtime-2`
channel and does not change the selected member's text model.

The console requirements are stricter than the existing stdin integration:

- Bind voice to the agent explicitly selected when the user starts voice.
  Navigating to another chat must not retarget an existing voice channel.
- Start with a **summary** of that background agent's current context, not a
  replay of its whole transcript and not an arbitrary suffix of messages.
- Keep the same existing background agent usable through normal text input
  while voice is connected.
- Run delegated work against that existing background agent, rather than
  silently creating a different executor with a forked context.
- Terminate after fifteen minutes of voice silence. Only real microphone/model
  audio activity resets the timer, including model output before speaker mute.
  Ordinary text sends, polling, transport keepalives, and silent audio frames
  do not. Inactivity is not proof of completed playback.

### Released API gaps

**Historical survey (Meerkat 0.8.38).** These are observations of the exact
crates.io 0.8.38 sources, not assumptions from the older local Meerkat checkout.
They predate the current 0.8.40
composition and are not claims that these owner seams are still missing:

1. `meerkat::session_runtime::SessionRuntime::open_live_channel_with_execution_identity`
   takes a `LiveSeedWindow`, not a summary provider or summary value. It calls
   `live_open_projection_for_session`, which snapshots the canonical session
   and retains either all projected messages or the existing compaction summary
   plus complete tail turns. A character window does **not** produce a summary
   of the current context.
2. `meerkat/src/experimental_gpt_live.rs`'s prepared factory
   `open_live_adapter` serializes those `seed_messages()` into
   `{"canonical_messages": [...]}` for `session.commentary.append`, excluding
   System/SystemNotice rows. Catalog `session_instructions` describe stable
   voice behavior; they are not a per-session summary injection seam.
   `RealtimeSessionOpenConfig::with_seed_messages` exists at the lower factory
   layer, but no summary transformation is composed into the shared strict
   host. Replacing this projection needs a deliberate upstream/host contract
   preserving the snapshot lease, canonical cursor, and recovery semantics,
   not a browser-authored transcript or forged compaction-summary row.
3. `meerkat-mob-mcp/src/live_delegation.rs::start_admitted_delegation`
   unconditionally constructs `DelegationExecutionRequest::new_live(...).
   with_durable_fork(source_identity, Some(committed_message_count))`.
   It obtains generated worker-start/consequential-effect authority, starts a
   durable executor fork, and owns result reconciliation/cleanup. There is no
   existing-background-member selection in the registered client-context
   coordinator. Copying this coordinator into Meta or sending a second
   untracked text turn would create competing execution authority.
4. `rpc_gateway` installs `LiveRpcHandler` only in its stdin dispatcher and
   publishes sanitized assistant-output addresses through the stdin callback
   bridge. `http_console` advertises no live atom in its
   `feature_capabilities` array (only non-live features such as
   `flow_role_targets`) and has no strict live handler, response-delivery
   custody, or public-observation delivery acknowledgements. `meta_gateway`
   does not register the public live host.

At that baseline, the browser had to treat voice as unavailable until those
boundaries were implemented and verified. The safety constraints still apply:
do not enable voice merely because `OPENAI_API_KEY` is set,
advertise the live execution capabilities on HTTP, switch a member's text
model, fall back to a legacy live endpoint, compact/rewrite the background
transcript to manufacture a seed, or report a fork as the existing agent.

### Authenticated target readiness (current)

Untargeted `/console/experience` never probes credentials or scans the roster
for voice readiness. Its `voice.available` stays false; a configured,
authenticated host additionally publishes
`voice.readiness_method: "meta/console/voice/readiness"` as discovery, not an
authorization grant.

Call `meta/console/voice/readiness {identity}` for the selected target. The
result is exactly `{identity, available}`, echoing the requested identity, and uses the shared
`probe_execution_readiness` authority: current durable source, configured
binding, AuthMachine and credential resolution, with no summary, channel
reservation, or Live provider-session open. Credential acquisition/refresh can
perform I/O; the HTTP observation is bounded to five seconds.

Require an exact identity echo and combine the result with view/send affordances to show the
action. The UI may retain it briefly (about five seconds) for display, but the
controller calls readiness freshly before acquiring the microphone or opening.
Failure, malformed data, a missing/mismatched identity echo, an obsolete target request, or false is unavailable. Open
independently revalidates. Keep hang-up controls after readiness revocation.
An existing voice on the same exact target does not make authentication
readiness false: its live grant is reused and credentials are still rechecked.
The one-voice/open exclusivity fence is enforced separately by open.

### Gateway configuration

Compile with `openai-live` (the release binary feature). In `rpc_gateway`, supply
`runtime_options.console_voice` and `runtime_options.auth_config` with persistent
state. In standalone `meta_gateway`, supply top-level `console_voice`,
`auth_config`, and `persistent_sessions: true`.

The console voice registration uses the shared public profile shape:

```json
{
  "principal": "alice@example.com",
  "realm": "voice",
  "auth_binding": {"realm": "voice", "binding": "openai"},
  "voice": "marin"
}
```

The principal must equal the resolved authenticated console principal (email
when present, otherwise JWT `sub`). The referenced
binding must exist in the gateway's Meerkat config and resolve through the
normal factory to an eligible OpenAI API credential. Optional
`session_instructions` are trusted, host-owned voice guidance, not a replacement
for source summarization.

`auth_config` is the existing explicit JWT contract (`provider: "jwt"`,
server-side `shared_secret`, optional issuer/audience, and `email_allowlist`).
JWT authentication and OpenAI authentication are separate gates; neither
credential is sent to the browser by the voice endpoints.

`rpc_gateway` rejects combining `console_voice` with `openai_live` or
`experimental_live`: those are alternative strict registrations of the same
public GPT Live door. Ordinary `live: true` may be registered beside
`console_voice`; see the next section. Console voice does not alter ordinary
agent text RPCs.

### One shared live owner: console voice beside the external live channel

A gateway that registers both `runtime_options.console_voice` and
`runtime_options.live` composes ONE live context (one adapter host, one WS
state, one provider registration) and mounts two doors onto it: the
authenticated console voice controller and the external door (the live
WebSocket router at `{base}/live/ws` plus the stdio `meta/live/*` RPCs).
Meerkat keeps channels per session and couples nothing across members, so the
policy that only one voice path is active per gateway is Meta's:
`live_wiring::LiveOwnerArbiter`, held by the shared `GatewayLiveContext`.

Arbitration is "latest engaged wins". Each door engages the arbiter before it
opens, carrying the close sequence it would use on itself:

- A console `meta/console/voice/open` closes an active external channel
  through the external host's generated close. The external client sees its
  socket end; `meta/live/status` and `meta/live/close` for that channel
  answer `"close_reason": "superseded_by_console_voice"` (a closed channel the
  arbiter ended reports `status: closed` with the reason instead of demanding
  a phase receipt), and the stdio gateway emits
  `meta/live/superseded {owner, identity, channel_id, reason, superseded_by}`
  immediately.
- An external `meta/live/open` closes an active console call through the
  console slot's ordinary teardown. Every later console request for that call
  (`replacement`, `context_status`, `activity`) fails with kind
  `voice_superseded` and `data.reason = "superseded_by_external_live"`; the
  browser ends the call with "Voice moved to the external live channel". The
  open result carries `"superseded": {"owner": "console_voice", "identity",
  "channel_id"}`.
- `meta/console/voice/readiness` answers
  `{identity, available: false, reason: "external_live_active", holder:
  {identity, channel_id}}` while the external door holds the path. Plain
  unavailability keeps the old two-field shape.
- The same door re-engaging for the same member (a console reopen, the device daemon
  reopening) is not a preemption and announces nothing, but it never leaves two
  channels live: a previous channel still bound is closed as
  `replaced_by_same_owner` (reported through `close_reason` and the console's
  `voice_superseded` error) before the new open proceeds, and an open still in
  flight closes its own channel when it completes. The console switching to
  another agent is a different owner and closes the previous call, matching the
  browser's close-before-switch.
- A holder whose bound channel already ended outside both doors (a dropped
  external WebSocket, which meerkat-live closes by itself; a console call that
  died with its provider) is released, not trusted: readiness and the next
  engagement ask the machine whether the channel is still active before they
  report or close it, so a path nobody is using can never lock the other door
  out until a restart.
- A loser's close that fails leaves that owner in place and fails the newcomer
  closed (`voice_host_failed` on the console side, an internal error on the
  external side). There is never a silent double owner.

Neither door refuses. `voice_busy` remains reserved for a pending teardown of
the same console request and is not used for cross-owner arbitration.

Meerkat admits live opens one at a time per gateway process and queues none.
An open keeps that admission until its provider has acknowledged its seed:
for a console voice call, through the browser's WebRTC offer and the
provider's answer. A console voice open made during another call's setup is
refused before anything opens, as the retryable `voice_open_busy` on the
console side and `-32054` on `meta/live/open`. Open again, with backoff. A
console retry carries a new `request_id`: `meta/console/voice/open` answers
a repeated `request_id` with the refusal it recorded for it, unlike
`voice_busy`, whose retry repeats the same request. The open succeeds once
the other call's setup has finished.
A `-32054` error carries `data.kind: "live_open_busy"` and
`data.retryable: true`, so `meta/live/open` clients can branch on the kind as
they do for `-32050`.

Library embedders constructing a `MobBootstrapSpec::new` must install the
builder's shared `default_mob_tools` slot with `with_agent_mob_tools`, after
session-runtime/workgraph wiring. Console delegation must use that owning Mob
MCP state, not a second state constructed after bootstrap. The standalone
gateway installs this seam when console voice is configured.

Session-service decorators must also forward the feature-owned
`MobSessionService::commit_live_delegation_final_transcript` hook unchanged,
including the same `MeerkatMachine` reference. This hook is now required when
Live is enabled: previously its default refusal let voice connect while
preventing spoken requests from reaching background execution. The owner
commits the final transcript and projects its Live provenance to that machine.
The pre-build and after-create decorators forward this hook and the Live
bridge eligibility, snapshot, and operation hooks to the owning service.

### HTTP request fencing

Both gateway HTTP apps install a console request controller; without the above
configuration it cannot enable voice or open a provider:

- `meta/console/voice/open {identity, request_id}` returns the raw strict
  pending handle from the composed shared host; an
  unconfigured host returns `-32050`, `data.kind: "voice_unavailable"`.
- `meta/console/voice/close {identity, request_id}` fences that
  request ID for its authenticated principal, including when close arrives
  before open, while its closed slot is retained as described below. It returns
  `{phase: "closed"}` only after any late successful open has been closed
  through the shared host.
- Open requires a concrete authenticated console principal, view/send grants,
  a writable console, and fresh host readiness. Close requires the same request
  owner but remains usable after view/send or readiness revocation.
- While its slot is retained, repeating an open request never opens a second
  channel. Reusing its ID for a different agent is a conflict; starting another
  request for the same principal while an earlier channel remains open/closing
  is refused.
- Cleanup outlives dropped HTTP request tasks. A ten-second close observation
  timeout returns `voice_busy`, not a success or an invented terminal phase;
  retry the same request. Shared-host cleanup failure remains retryable.
- Closed slots, including cancellation tombstones, are lazily reaped on new-ID
  admission. They become eligible after ten minutes from the registry's recorded
  closed-observation time (set immediately for close-before-open tombstones,
  otherwise when the reaper first observes closure), not on a background timer
  precisely ten minutes after close. They may be reclaimed sooner to retain
  only the newest 32 closed slots per principal or to free the 4,096-entry total
  registry. Opening, active, and failed-cleanup slots are not evicted by this
  reaper; if capacity remains exhausted, new IDs are refused.
- Duplicate-open and cancellation guarantees last only while the slot is
  retained. Clients must never reuse a closed request ID: after eviction, an
  old ID can be admitted as new work. This does not change the instruction to
  retry the exact in-progress close after an observation timeout.

These are product request/retry mechanics, not replacement live authority.
The host retains the upstream channel and publication custody behind each
request. The complete request-owned HTTP channel subset is
`meta/live/playback_owner/register`, `meta/live/playback_owner/revoke`,
`meta/live/status`, `meta/live/close`, `meta/live/refresh`,
`meta/live/interrupt`, and `live/webrtc/answer`. Each requires the exact
identity and channel of the authenticated owner's current request, in addition
to the applicable strict receipts. Raw `meta/live/open`,
`meta/live/send_input`, `meta/live/commit_input`, `meta/live/truncate`,
and `meta/live/playback_complete` are not HTTP console fallbacks. Guessed
playback completion and provider-native identifiers are not alternative paths.

Browser close gates microphone transmission and speaker gain immediately but
keeps the muted WebRTC connection alive while the shared host drains provider
acknowledgements. It releases all local resources after confirmation or the
five-second browser deadline; page unload and disposal release them immediately.
A missing server confirmation still blocks a new voice request and retains the
exact close for retry.

In client delegation, typed composer input continues through ordinary background
agent admission. The shared canonical-context mirror carries those user-data
updates and resulting answers into Live. It must not turn typed input into
trusted instructions or invent a native Live user-text command.

The console selects the upstream `ProviderManagedUnmeasured` playback policy:
continuous browser WebRTC has no trustworthy per-output consumed-completion
signal. Assistant speech is observation-only, not a claim that the user heard
it; the shared owner releases staging according to that explicit policy.
Provider-emitted assistant transcript is retained canonically with explicit
`UNMEASURED` provenance. Replacement summaries consume those observed rows and
describe what the agent produced or observed, never asserting that the user
heard, accepted, or acted on it. Live-origin cursor advancement remains
upstream-owned and does not echo the speech back to the provider.
Neither `response.done`, a media-element event, nor analyser silence is used
to manufacture `playback_complete`. Public output callbacks remain
loss-intolerant when the owner emits them, but are not guaranteed once per
spoken utterance.

After `live/webrtc/answer`, apply the remote SDP and then call
`meta/console/voice/answer_received {identity, request_id, channel_id}`.
Its `{accepted: true}` response settles the retained upstream HTTP answer
publication custody. Only then poll strict status for activation and release
media gates. Open publication is acknowledged by the subsequent exact
pending-receipt owner registration. Failed/disconnected setup retains cleanup
ownership until request-scoped close.

`meta/console/voice/activity {identity, request_id}` returns `{accepted: true}`. Report fresh,
measured non-silent audio, debounced to at most once per five seconds; never send
ordinary text or a keepalive as activity. The backend independently enforces
the 900-second silence window and retries shared-host cleanup. An observation
timeout does not pretend cleanup succeeded.
The silence window starts at the first successful activation/answer custody,
not during auth or summary production. Replacement acknowledgements preserve
the existing deadline. Unactivated pending setup has a separate two-minute
cleanup deadline beginning only after the pending handle is produced; it does
not consume any of the active voice's fifteen minutes.

Gateway shutdown first stops HTTP admission, then fences and drains voice
requests before runtime authority cleanup. Its additional ten-second bounded
phase is included in the SDK gateway's advertised shutdown horizon.

### No console playback-receipt transport

The console's `ProviderManagedUnmeasured` path settles bookkeeping internally
and deliberately does **not** publish actionable `AssistantOutputAvailable`
handles. Console speech needs no output-id polling, queue ACK, or
`playback_complete`. Do not add an observer-polling dependency to activation or
ongoing audio.

There are no new console `outputs/poll` or `outputs/ack` endpoints. The existing
stdin measured-host publisher contract is unchanged. The console supplies a
rejecting publisher guard: an unexpected request for a played-output
publication is an error, never a fabricated delivery receipt.

The browser installs its actual media consumer before registering the media
owner. SDP publication still requires `answer_received`; this is distinct from
playback. Dialogue continuity is retained by the upstream feature/document
owner, not replayed from a browser buffer. No provider event or analyser signal
is promoted to a heard/completed utterance.

### Provisional assistant captions

Under `ProviderManagedUnmeasured`, Meerkat commits one canonical assistant row
per provider turn segment. The row lands when the segment seals: at the turn
end (gpt-live-1 has no assistant completion event, so a speaker change or
close ends the turn), at a typed between-speech boundary, or at channel close.
The console covers that gap with provisional captions.

The live host installs a `PublicGptLiveProvisionalCaptionSink` on its public
authority. Meerkat calls it synchronously, in delta order, outside its locks.
Each caption names the session, the channel, the segment item id, and the
segment's whole text so far. A retraction names a segment that no committed
row will replace. The sink never waits on the browser. It replaces the item's
entry in a bounded per-channel buffer (coalesced per item, oldest evicted
first) and wakes readers. Captions for sessions without a console call are
dropped.

`meta/console/voice/captions {identity, request_id, channel_id, after,
wait_ms}` reads that buffer with the same exact `{identity, request_id,
channel_id}` scope as `context_status`. It answers `{identity, request_id,
channel_id, cursor, captions}` with entries newer than `after`, waiting up to
`wait_ms` (at most 15 s) for one. Each entry is `{kind: "caption", item_id,
text}` or `{kind: "retracted", item_id}`. A closed call answers
`voice_closed`, and a stale or replaced channel answers
`voice_request_conflict`.

The browser upserts `liveSpeech` keyed by `("assistant", item_id)` with the
caption's text and drops the item on a retraction. The chat pane hides a
caption once a committed row in the same session and channel lists its item
id in `realtime_origin.provider_item_ids`. Pairing never uses text or time.
Data-channel `session.output_transcript.*` events name no committed item, so
they never create assistant live rows. User live rows still come from
item-keyed input transcript deltas. Captions are display state only: a failed
read is retried and never affects the call, and close or a replacement channel
clears them.

### Replacement discovery must survive closure of the old channel

Canonical context updates and normal delegation results are mirrored in place
by Meerkat's sideband owner. The browser does not replay them. Ambiguous
delivery can instead produce a fresh replacement transport; the shared
`pending_replacement_required(session_id)` returns the same pending replacement
until its answer binds. The browser must negotiate a new peer/owner/answer and
wait for activation, without changing the background identity.

The #1117-based candidate adds an opaque `pending_receipt` to each replacement
variant. Meta's current SDK replacement parser still expects the older
`LiveChannelHandle` shape without that receipt. Its strict RPC preflight also
requires the old channel's active receipt, which is not sufficient once
recovery has closed that old channel. The console adapter therefore uses
`meta/console/voice/replacement {identity, request_id}`. It returns
`{required: false}` or
`{required: true, reason, replacement, canonical_seed_cursor}`, where
`replacement` is the full strict pending handle. Install a new peer and owner,
answer/acknowledge/activate it under the same pinned request identity, and
suppress old-peer loss callbacks during intentional replacement. Neither
receipt invention nor repeatedly calling the old active-only SDK API is valid.

### Existing strict wire contract to preserve

The following is the underlying strict stdin handler contract. Console starts
through its request-fenced open wrapper and uses only the channel subset in
[HTTP request fencing](#http-request-fencing). In particular, HTTP console
discovery uses `voice.readiness_method` and authenticated per-target readiness,
not generic Live atoms in its `feature_capabilities` array (which carries no
live atom). It does not expose guessed playback completion or truncation as
browser fallbacks:

| Method | Request fields beyond JSON-RPC envelope | Result |
| --- | --- | --- |
| `meta/capabilities` | none | Require both `live.execution_identity.v1` and `live.execution.client_context.v1` in `feature_capabilities`. |
| `meta/live/open` | `identity`, `transport: "webrtc"`, `execution_identity: {version: "v1", profile_id: "openai.gpt-live-1.client-context.v1"}` | Pending handle: `channel_id`, `target_identity`, `execution_mode`, `pending_receipt`, `transport`, `capabilities`, `continuity`, and `advisories` when the open went ahead without something (the voice-first staged prompt item under [meta design](#meta-design)). |
| `meta/live/playback_owner/register` | `identity`, `channel_id`, `pending_receipt` | `channel_id`, `readiness_receipt`. |
| `live/webrtc/answer` | `identity`, `channel_id`, `pending_receipt`, `readiness_receipt`, `token`, `offer_sdp` | `answer_sdp`; publication custody must succeed before activation. |
| `meta/live/status` | `identity`, `channel_id`, exactly one of `pending_receipt` or `activation_receipt` | `phase: "pending"`, `"active"` with `handle`, `"revoked"`, or `"closed"`. Active handle carries `channel_id`, `target_identity`, `execution_mode`, `activation_receipt`. |
| `meta/live/close` | `identity`, `channel_id`, exactly one phase receipt | `status` from the shared close owner. |
| `meta/live/playback_owner/revoke` | `identity`, `channel_id`, `pending_receipt`, `readiness_receipt`, plus `activation_receipt` if active | `phase: "revoked"`. |
| `meta/live/playback_complete` | `identity`, `channel_id`, `activation_receipt`, `output_id` | `status: "completed"`. |
| `meta/live/truncate` | `identity`, `channel_id`, `activation_receipt`, `output_id`, `audio_played_ms`, optional `reported_playback_prefix` | Shared typed truncation result. |

Prepare and gate microphone/output before registering the playback owner.
Apply the answer while media remains gated. Release media only after status
returns the generated active handle. The open bootstrap carries a single-use
token and the answer method; it never carries a provider API key.

Client-context delegation, canonical context mirroring, final-transcript
reconciliation, and delegation-result acknowledgements are server-side Meerkat
sideband responsibilities. The browser must not synthesize
`session.commentary.append` or `delegation.context.append`, choose delegation
targets from provider datachannel events, or turn a provider item id into an
`output_id`. For measured-host playback, the sanitized host observation supplies
`{channel_id, output_id, content_index}`. Any HTTP transport exposing that
observation must deliver and acknowledge it under the exact current
principal/channel fence before the provider pump considers it published. The
current unmeasured console path does not expose actionable output observations;
see [No console playback-receipt transport](#no-console-playback-receipt-transport).

Any HTTP composition of this strict surface must authenticate a concrete
principal, check `agent.view` and `agent.send` on the canonical durable target,
revalidate the current member/session binding, and scope receipt and observation
operations to that principal and target. No missing-principal path may become
`host_trusted_stdio`. An HTTP JSON serialization success is not evidence that
an answer or output observation reached the browser; response-loss cleanup and
bounded acknowledgement expiry are required.

### OpenAI authentication is an additional availability gate

The console must not offer voice or ask for microphone permission unless the
server has positively resolved an authenticated, usable OpenAI binding for the
selected live execution profile and the requesting principal/agent. An
environment-variable check, a model-catalog entry, or the presence of
`runtime_options.openai_live` is not that evidence.

In 0.8.38,
`ExperimentalGptLiveOpenAuthority::execution_feature_capabilities()` returns the
public capability atoms for a public registration without inspecting current
credentials. Actual per-target binding authorization and credential resolution
occur later in `prepare_open` through
`AgentFactory::resolve_public_live_binding_for_identity` and
`resolve_public_live_target`. Therefore those generic feature atoms alone
must **not** enable a console microphone affordance.

A console-ready projection needs a separate per-principal, per-target readiness
result from the same configured upstream credential authority used by open.
Missing, expired, revoked, wrong-provider, unusable, or unauthorized bindings
must fail closed before microphone acquisition; an open must recheck readiness
rather than trust an earlier browser capability response. Provider credentials
and account secrets never cross that projection.

### Development pin and release rebinding

**Historical checkpoint (2026-09-16), superseded by the registry baseline at the
top of this page.** The development lineage and qualification evidence below
are retained; they are not instructions to restore a patch or downgrade.

The requested Meerkat **0.8.39** / Meta **0.8.36** baseline was incorporated.
On 2026-09-16, crates.io published Meerkat 0.8.39 from
`ad39733a00743723c2227a43a7557cc3f2c6344f`, but that crate does not contain
`LiveContextSummaryPolicy` or `ProviderManagedUnmeasured`. Meta main then
contained the 0.8.36 release commit. Registry publication and GitHub release
listings can lag each other; version labels alone are not API evidence.
Meerkat [lukacf/meerkat#1117](https://github.com/lukacf/meerkat/pull/1117), inspected at
`890e3e71cc1b68bd0fe88198f2e1b6717dd2a76d`, changes playback settlement,
cold runtime restoration, close draining, and canonical history replay through
native `session.input`, but does not by itself provide the additional console
owner seams.

The development lineage rooted at `0b36e303e85674ebe85b2a06160db292e3b1a636`
adds summary seeding, existing-member execution, authenticated readiness, and
truthful unmeasured dialogue retention. The reviewed descendant additionally
fences explicit receipt-close against delayed replacement preparation and
registration, without cancelling a fresh same-agent call.
The initial shared-owner pin was
[`34838b2d0e5c63c9206b79c3cae48d9961e99e05`](https://github.com/lukacf/meerkat/commit/34838b2d0e5c63c9206b79c3cae48d9961e99e05),
tracked by [lukacf/meerkat#1124](https://github.com/lukacf/meerkat/pull/1124).
The published console repair checkpoint uses
[`d7318919e4ce29ce96e05f8466bcbb8d4baabb0c`](https://github.com/lukacf/meerkat/commit/d7318919e4ce29ce96e05f8466bcbb8d4baabb0c),
which adds failed-provider cleanup, per-message context provenance, and
post-commit mirror notifications for RPC and mob-owned executors.
That checkpoint patched the full Meerkat family to this exact HTTPS Git
revision. Subsequent human-input and concurrent-context work was qualified
against isolated immutable development snapshots; those are not final
distributable pins. At that checkpoint, rebinding to the then-published Meerkat
0.8.39 would have removed required owner seams, so a development pin remained
necessary. Those seams are now supplied by the exact registry 0.8.40 family in
this checkout. Keep dependency-family consistency and API/voice qualification
as release gates; neither historical development tests nor the later registry
repin establish a new successful real-provider audio run.

### Additive upstream acceptance requirements

These requirements from the development composition remain acceptance gates
for future dependency changes:

1. **Summary seeding at an exact snapshot.** A host-supplied summary producer
   operates on the authoritative context snapshot while Meerkat retains the
   projection lease and canonical cursor. The default `BeforeOpen` policy
   refuses the open on failure, rather than silently sending raw history.
   The summary does not rewrite or compact
   the background session. Reopen/recovery preserves the same summary policy;
   incremental canonical context is not accidentally omitted or replayed.
2. **Existing-member execution strategy.** Client-context delegation selects
   the already-bound background member through shared generated admission,
   idempotent input delivery, final-transcript correlation, result publication,
   and cancellation/recovery. It must not spawn a durable fork, duplicate the
   user transcript, block independent text submission, or change the member's
   normal model/tool policy. No Meta-owned replacement coordinator.
3. **Authenticated readiness without provider opening.** A secret-free,
   current per-target projection uses the same fixed public profile, selected
   configured OpenAI binding, credential resolver, and AuthMachine authority
   as the real open. It distinguishes usable admission from merely configured
   OAuth/token status. Missing/expired/revoked/unauthorized credentials refuse
   before browser microphone acquisition, and open independently revalidates.
4. **Preserved delivery custody.** Existing pending/active receipts, answer
   publication custody remain usable by an authenticated HTTP host. The
   existing measured-host publisher contract stays unchanged. Caller
   disconnect, failed response publication, stale member generation, and owner loss close the
   exact binding without orphaned authority. A late successful open after
   request cancellation must be discoverable and closeable through the same
   owner; local fetch cancellation is never evidence of server termination.

Upstream tests must cover an active text turn racing voice open/delegation,
context changes during summarization, summary failure, cold resume, restart or
generation replacement, duplicate/ambiguous delivery, and cancellation before
the open response is delivered. Meta separately owns authenticated HTTP
request-id fencing, answer delivery acknowledgements, the console
silence policy, and agent-selection UI policy.

Meta's prepared summary producer uses the existing typed `LlmClient` request
path with no tools. It renders the authoritative snapshot as data, requests a
factual context summary, enforces the UTF-8 output budget, excludes reasoning
blocks, and rejects incomplete, truncated, empty, non-text, or oversized output.
It neither modifies the source transcript nor substitutes its own snapshot
authority. The adapter to `LiveContextSummarizer` must use
`snapshot.llm_identity()` through the gateway's configured factory, rather than
re-reading a potentially changed text model after acquiring the snapshot.
`LiveContextSummaryPolicy` owns snapshot, timeout, and stale-input admission,
including the exact body/rewrite/session/cursor witness and canonical System
projection, not merely a message count.
Summary-producer tests alone are not evidence of a composed summary-seeded
Live open; actual shared-host and native-audio acceptance are separate gates.

### Nonblocking console context bootstrap

The console opts into `LiveContextBootstrapMode::Concurrent`. Audio admission
and receipt activation do not await summary generation. Meerkat owns the
captured canonical prefix separately from provider-delivered knowledge, orders
its quiet context delivery with newer conversation updates, and fences or
cancels jobs when their channel closes or is replaced. Meta does not mint a
delivered cursor, insert a synthetic user turn, or substitute instructions
for summary authority. Non-console callers retain the `BeforeOpen` default.

`meta/console/voice/context_status` reads the retained shared-owner custody
with an exact `{identity, request_id, channel_id}` scope. Its response echoes
that scope and projects `context_preparation` as `not_requested`,
`preparing` with a `capturing`, `generating`, or `delivering` stage,
`provider_acknowledged`, or `failed` with a typed reason. It does not change
the strict pending/active handle schemas. Stale, foreign, closed, or replaced
channel scopes refuse rather than reporting another call's context.
`not_requested` means no concurrent preparation job, not proof that the
provider has no preloaded history. Published Concurrent handles already have
preparation staged; they cannot transiently return `not_requested` before
their job starts.

The browser observes preparation independently after audio activation.
Preparation or failure never mutes an otherwise active channel. A status-read
failure is visibly unknown and retried, not converted into provider success
or summary failure. Each read is bounded; switching, closing, or recovery
cancels the observer and ignores its late responses, including same-request
channel replacements. Status reads and summary completion do not refresh the
15-minute audio-silence deadline. Provider acknowledgement means acceptance
of supplied context, not demonstrated recall or completed speech.

## Upstream shape (meerkat 0.7.25, surveyed)

Historical survey with later compatibility notes: version-specific claims
below describe the named releases, not current registry API availability.
The current console composition is described above.

- `meerkat-live` is deliberately embeddable: `LiveAdapterHost` (the
  transport-side orchestrator) + an axum WS router
  (`live_ws_router(Arc<LiveWsState>)`, path `/live/ws`) that mounts on any
  existing `Router`. It has NO meerkat-runtime dependency; canonical
  semantics arrive through two injected traits: `LiveProjectionSink` and
  `LiveToolDispatcher`. WebRTC optional behind the `webrtc` feature.
- Experimental GPT Live canonical projection is shared by Meerkat's generic
  `ServiceLiveProjection<B>`, which implements `LiveProjectionSink`,
  `LiveChannelCloseFeedback`, `LiveChannelStatusFeedback`, and
  `LiveWsTokenAuthority`. Meta composes this facade directly and does not
  port experimental transcript, playback, or machine-authority sequences.
  Published Meerkat 0.8.26 still exposes only its Factory-bound projection,
  so non-experimental builds retain Meta's preexisting ordinary websocket
  projection behind an exclusive compatibility cfg. The two paths cannot be
  compiled into the same build.
- Lifecycle authority is machine-owned: every open/close/status/token step
  requires a non-forgeable authority minted by `MeerkatMachine` live
  methods (`resolve_live_open_admission`, `resolve_live_close_result`,
  `record_live_websocket_token_issued`,
  `resolve_live_websocket_token_admission`, ...). All present in published
  meerkat-runtime 0.7.25 under the `live` feature (meta already enables
  it).
- Session-scoped seams the sink needs are inherent methods on
  `PersistentSessionService<B>`: `append_external_user_content`,
  `append_external_assistant_output`, `append_realtime_transcript_event`,
  `dispatch_external_tool_call`, `record_live_terminal_error`,
  `record_live_output_audio_degraded`. Realtime transcript events commit
  into the ONE canonical Session history through the same append-only save
  path as normal turns (so the Bug B-2 rollback fix in
  `ContinuitySessionStoreAdapter` covers live turns too).
- Ordinary live compatibility tool dispatch:
  `LiveToolDispatcher::dispatch_live_tool_call(session_id, call)` ->
  `dispatch_external_tool_call` -> the session agent's normal external-tool
  dispatch. The gateway's `CallbackToolDispatcher`, composed recorder tools,
  and gating apply to those ordinary live turns unchanged. This does not apply
  to experimental GPT Live ClientContext, whose voice endpoint has no direct
  tool dispatcher and uses a distinct durable executor fork.
- Tokens: single-use, 60s TTL, pinned to (token, channel). Bootstrap
  returned by open: `{channel_id, transport: {type:"websocket", url,
  token}, capabilities, continuity}`. Continuity is TranscriptOnly in
  practice (no provider-native resume ships).
- Credentials resolve PER OPEN via
  `AgentFactory::build_openai_realtime_session_factory(config_source)`
  where `RealtimeCurrentConfigSource` is just `async fn current_config() ->
  Config`. Facade features required: `live` + `openai-realtime`. Only
  `gpt-realtime-2` (OpenAI) is realtime-capable in the shipped catalog.

## meta design

Original design and versioned compatibility history for
`crates/meerkat-meta/src/live_wiring.rs`; the ordinary-stdin versus console-HTTP
boundary in item 5 is clarified for the current implementation. Older
published-version limitations below do not require a development pin today.

1. **Projection ownership** - experimental builds compose the shared Meerkat
   `ServiceLiveProjection<B: SessionAgentBuilder>` facade with the gateway's
   existing persistent service and `MeerkatMachine`. It is the single owner
   of experimental canonical transcript projection, assistant-output target
   admission/bind, playback completion, truncation and Unmeasured settlement,
   close/status feedback, and token authority. Stock 0.8.26 builds compile
   only the preexisting ordinary `GatewayLiveProjectionSink<B>` compatibility
   path until the generic facade is available in the minimum published
   dependency.
2. **`GatewayLiveToolDispatcher<B>`** — `dispatch_external_tool_call` on
   the service.
3. **`GatewayLiveContext`** — `{host: Arc<LiveAdapterHost>, ws_state:
   Arc<LiveWsState>, session_factory: Arc<dyn RealtimeSessionFactory>,
   ws_base_url}` built by `attach_live(...)` in the gateway (persistent
   mode only — live needs the runtime-backed service). The gateway merges
   `meerkat_live::live_ws_router(ws_state)` onto the reference app router,
   so the live WS shares the existing HTTP listener/port (the downstream app's
   `app.py` proxy or direct LAN access both work).
4. **Credential source**: `EnvRealtimeConfigSource` implementing
   `RealtimeCurrentConfigSource` by returning the gateway's effective
   `Config` (same `Config::default()` the agent builds use). Per-open
   resolution then rides the session identity's auth binding or the
   provider default (env `OPENAI_API_KEY`), matching text-model behavior.
   Embedders with real config stores can swap the source later.
5. **Ordinary RPC surface** (unified stdin, not the full console HTTP surface):
   `meta/live/open`,
   `meta/live/status`, `meta/live/close`, `meta/live/refresh`,
   `meta/live/send_input`, `meta/live/commit_input`,
   `meta/live/interrupt`, `meta/live/truncate`. Params accept an
   IDENTITY TARGET —
   `{identity: "device"}` or `{member_id}` or raw `{session_id}` —
   resolved via `resolve_bridge_session_id` + the roster `agent_identity`
   label fallback (the same canonicalization class as
   `/agents/{id}/events` and `cross_mob/peer_info`). Handlers are ports of
   `meerkat-rpc/handlers/live.rs` against `GatewayLiveContext`. Methods
   answer `-32050 live_unavailable` when the gateway has no live context
   (ephemeral mode, or feature disabled).
   The original full-console proposal is superseded: HTTP opens through
   `meta/console/voice/open` and supports only the explicitly enumerated
   request-owned channel subset in [HTTP request fencing](#http-request-fencing).
   The ordinary `identity`/`member_id`/`session_id` alternatives are not
   interchangeable HTTP authorization targets.
6. **Realtime model selection (v1)**: the member session's model decides
   (profile `model = "gpt-realtime-2"` for a voice-first member). For
   members whose text model differs, `meta/live/open` accepts an
   optional `model` override forwarded into `RealtimeSessionOpenConfig`;
   a per-profile `realtime_model` map can ride `runtime_options.live` in a
   follow-up once field usage settles. (Deliberately NOT a mob.toml
   profile field — profiles are upstream schema.) For members whose text
   PROVIDER differs too (the downstream app: Anthropic text profiles opening the
   OpenAI realtime lane), `meta/live/open` also accepts a strict
   optional `provider` paired with `model`: an unrecognized provider name
   is a typed invalid-params error (never a silent fallthrough), the
   (provider, model) pair is applied to the channel identity before the
   B19 precheck and machine admission, and when the selection differs
   from the member's inherited provider the inherited provider-specific
   auth binding is cleared so the selected provider's configured default
   credential resolution applies. Omitting `provider` keeps the previous
   behavior byte-identical. Both `model` and `provider` are CHANNEL-scoped:
   they mutate only the per-open `RealtimeSessionOpenConfig` projection
   (the member's durable identity is read via
   `live_session_llm_identity`, never written on this path), so channel
   close reverts by construction and `live/refresh` re-projects from the
   durable session.
7. **Voice-first staged prompt**: a member created with its first turn
   deferred and its create-time prompt staged (`InitialTurnPolicy::Defer`
   with `DeferredPromptPolicy::Stage`) can be opened for voice before any
   text turn. Every `meta/live/open` path hands that prompt to the realtime
   model once, through the `RunStarted` hooks a turn carrying it would run,
   committed as the member's newest user row before the seed is read; a
   later open or first text turn finds nothing staged. The ordinary gateway
   delivers it itself, as Meerkat's ordinary open does, at the member's turn
   boundary, which its seed read waits for anyway: another holder of the
   boundary delays the delivery, and a turn in flight consumes the prompt
   itself. An `openai-live` gateway's opens go through Meerkat's own open
   projections, which deliver it too: the mirrored-channel and concurrent
   summary opens only try the boundary and never wait behind a running turn,
   and the ordinary and summary opens deliver at the boundary their seed
   read waits for. A member with nothing staged costs the open nothing. A
   hook that refuses the prompt keeps it staged for the first text turn and
   the open goes ahead without it: the open result (and the pending or
   replacement handle) carries `advisories: ["staged_prompt_refused"]`.
   `advisories` is left out when there are none, and its vocabulary may
   grow: the SDKs keep an advisory they do not know as the string the server
   sent.
8. **Gating**: `runtime_options.live = true | {ws: true}` opt-in on
   `meta/init` (default OFF). ABAC: live methods map to `agent.send` on
   the target member (console surface); stdin surface is host-trusted as
   usual.

## Experimental channel-scoped execution

The experimental surface is stricter than ordinary live compatibility.
It accepts only an identity-first durable member target. Raw `session_id`,
runtime `member_id`, mixed target forms, and stale aliases cannot acquire the
experimental capability. The nested `execution_identity` request is versioned
and strict. The caller does not select a provider-native delegation mode or
provide a Responses model, bridge instructions, or tool declaration.

The catalog-qualified shipping mode is `client_context`, backed by
`live.execution.client_context.v1`. It configures `delegation.type = "client"`
and sends no provider tools, Responses model, or Responses configuration. The
voice model decides when to delegate through GPT Live's defined client
delegation lifecycle. Meerkat then owns the canonical-transcript join,
durable executor fork, bounded result, and exact
`delegation.context.append` acknowledgement.

`function_bridge` remains dormant, independently gated vocabulary. Current
builds neither advertise nor select it because direct probes have not observed
the raw Responses function-call carrier or settlement lifecycle. It never
falls back silently to ClientContext.

Experimental open returns `PendingLiveChannelHandle`, not an active channel.
The pending receipt permits only playback-owner registration, status, WebRTC
answer under the resulting readiness receipt, and close. Activation mints a
distinct `ActiveLiveChannelHandle` with an opaque activation receipt. Refresh,
input, commit, interruption, truncation, replacement, and playback settlement
require that exact current active receipt. Playback-owner loss revokes active
authority before the provider can accept more effects.

The Python and TypeScript high-level connect methods install the gated media
owner, register readiness, answer, wait for generated activation, and only
then release media and return the active handle. Availability is not a
registry-versus-development distinction: `openai-live` compilation plus an
explicit registered and composed strict host is required for stdin execution
capability advertisement. Unconfigured builds remain unavailable. HTTP console
uses authenticated per-target `voice.readiness_method` discovery instead of
advertising those generic atoms.

## Public GPT Live registration (`runtime_options.openai_live`)

Meerkat 0.8.38 ships a public OpenAI Live path for the released `gpt-live-1`
catalog row behind the `openai-live` feature (meta forwards it as
`crates/meerkat-meta/openai-live`). It reuses the strict channel machinery above
(pending/active receipts, WebRTC answer, playback custody, client-context
delegation) but has no operator, Gate0 qualification, or factory identity:
the released catalog row, the host's configured OpenAI API-key binding, and
the compiled feature are the admission.

The gateway registers it through one explicit stdio object. Every field is
host-owned; callers cannot select a model, provider, or binding:

```json
"runtime_options": {
  "openai_live": {
    "principal": "user:alice",
    "realm": "home",
    "auth_binding": { "realm": "home", "binding": "openai-api-key", "profile": "alice" },
    "voice": "marin",
    "session_instructions": "optional trusted voice guidance"
  }
}
```

`principal`, `realm`, `voice`, and `auth_binding.{realm, binding}` are required
non-empty strings; `auth_binding.profile` and `session_instructions` are
optional (omitted or `null`). `auth_binding.realm` must equal `realm`, the
binding origin is always `Configured`, and unknown fields anywhere in the
object are rejected. The gateway fixes the execution identity to provider
OpenAI, model `gpt-live-1`, and that binding, then composes the public open
authority. Like `experimental_live`, the registration is independent of
`runtime_options.live` and does not mount the HTTP `/live/ws` route.

Capabilities advertise `live.execution_identity.v1` and
`live.execution.client_context.v1`. Callers select the platform profile
`openai.gpt-live-1.client-context.v1` (SDK constant
`OPENAI_GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID`) through the versioned
`execution_identity` request; it is a reserved id that hosts cannot rename or
override. The SDK builders expose it as `openaiLive(...)` /
`openai_live(...)`.

`runtime_options.experimental_live` (the private ChatGPT-brokered
`gpt-live-1-codex` path, feature `experimental-gpt-live`) is deprecated. It
still composes on top of `openai-live` for hosts that carry Gate0 evidence,
but configuring both `experimental_live` and `openai_live` in one init is
rejected at parse time.

## Images (meerkat 0.7.27, mobkit 0.7.32)

Still-image input rides the SAME transport and RPC surface: the wire chunk
`{kind: "image", idempotency_key, mime, data}` flows through
`meta/live/send_input` unchanged (the handler deserializes
`LiveInputChunkWire`, which gained the exhaustive `Image` variant), and the
SDKs add `live_send_input_image` / `liveSendInputImage` conveniences
mirroring meerkat's SDK signatures. `idempotency_key` is caller-stable
within the session — retries are exact-retry deduplicated by the runtime's
user-content identity lane, which also rides the open config
(`user_content_identities` / `user_content_tombstones` /
`transcript_rewrite_generation`) so reopened channels do not replay
committed images. The shared Meerkat projection forwards the transcript apply outcome
(0.7.27 API) so the host synthesizes the redacted image receipt only after
durable reducer application. Only `gpt-realtime-2` accepts image input in
the shipped catalog (capabilities carry `image_in`). As of meerkat 0.7.28
the catalog default model is `gpt-5.6-sol` (GPT-5.6 Sol/Terra/Luna added;
explicit GPT-5.5 pins stay honored) — realtime capability is unchanged.

## Field-reported additions (mobkit 0.7.32)

- **Host-trusted voice profiles (strict opens)**: `meta/live/open` selecting
  `execution_identity` and a host-registered public/experimental profile rejects
  caller-supplied instructions, execution-mode overrides, model, provider,
  tools, and Responses configuration. Unknown fields inside the nested
  `execution_identity` envelope, including capability claims, are rejected.
  Extra top-level capability fields such as `capabilities` or
  `feature_capabilities` are ignored, not rejected; they cannot grant authority,
  and capabilities remain host-owned. A named profile
  selects host-owned session instructions; it cannot introduce callback or
  direct-effect authority. The selected profile's qualified ClientContext mode
  is not configurable through the untrusted open request. Ordinary stdin
  compatibility opens, without `execution_identity`, instead accept
  channel-scoped `instructions`, `model`, a validated `provider` paired with an
  explicit `model`, and `turning_mode`. These ordinary controls are not console
  voice overrides.
- **Seed clamp (upstream ask 30, historical stopgap — replaced)**: Meta
  originally dropped whole canonical conversation messages oldest-first to
  bound long seeds. Current `runtime_options.live.seed_max_chars` (object form)
  supplies the gateway-wide serialized-character budget; per-open
  `seed_max_chars` takes precedence. Both ordinary paths forward that window
  through Meerkat's `LiveSeedWindow` projection authority. When the full
  projection does not fit, upstream selects an affordable existing compaction
  summary, if any, plus a contiguous suffix of complete conversational turns
  and reports explicit `Windowed` projection status for the reduced context.
  It does not generate a fresh summary or promise to retain every
  System/SystemNotice row. This character budget is not a universal provider
  token-limit guarantee. Console
  summary production is separate; see
  [Nonblocking console context bootstrap](#nonblocking-console-context-bootstrap).
- **`meta/live/truncate`** (was deliberately unported in v1): barge-in
  cleanup — truncate an assistant item at the client-tracked playback
  cursor. Same machine-authority choreography as the sibling command
  handlers; SDK conveniences `live_truncate` / `liveTruncate`.

## What ordinary live compatibility deliberately does not do

- WebRTC remains absent from the ordinary compatibility path. Experimental
  channel-scoped execution uses the separately gated WebRTC surface.
- Provider-native resume (upstream returns TranscriptOnly anyway).
- Console UI affordance (phase 2, with the SDK methods).
- A second listener/port: the WS mounts on the existing gateway HTTP app.

## Separate generic durable delegation

The ordinary agent-facing Mob tool roster also exposes `fork_off`. It is a
provider-neutral durable fork-plus-bounded-run operation for tool-capable
agents. It is not a GPT Live tool, imports no live protocol authority, and is
not required for voice to function. The experimental voice coordinator and
`fork_off` both reuse Meerkat's existing durable fork primitive through their
own typed callers.

## Test plan

- Sink unit tests: delta identity fail-closed, pending-turn drain keyed by
  response_id, terminal-error drain-all (ported assertions).
- A `FakeRealtimeSessionFactory` (scripted `RealtimeSessionEvent`s) driving
  an end-to-end open → observation → transcript-commit → close against a
  real persistent service + machine, asserting the member session's
  transcript contains the committed turn.
- Token admission: minted token admits once on the right channel; replay
  and cross-channel use rejected (machine-backed single-use).
- RPC: identity-target resolution parity (identity/member alias/session id
  spellings), live_unavailable off-state.

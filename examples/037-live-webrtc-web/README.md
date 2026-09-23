# 037 - Live WebRTC Web Smoke Test

Browser-based manual smoke test for Meerkat Live over WebRTC.

This intentionally exercises the long path:

```text
browser microphone + data channel
  -> Meerkat WebRTC transport
  -> meerkat-live LiveAdapterHost
  -> OpenAI live adapter
  -> oai-rt-rs
  -> OpenAI Realtime
```

It starts one live voice control session. It does **not** pre-create a mob.
Instead, ask the live agent to create one:

```text
Create a mob that reviews whether we should ship today with a lead, skeptic,
and release manager.
```

The live session has the real `meerkat-mob-mcp` agent tool surface enabled
(`mob_create`, `mob_spawn_member`, `mob_list`, profile CRUD, and related tools)
plus two host callback tools: one saves notes and one writes longer output to
the cockpit's text pane.
The cockpit shows transcript observations, tool requests, created mobs/member
status, saved notes, text-pane output, WebRTC state, data-channel state, and
manual live controls.

## Run

From the repository root:

```bash
./scripts/repo-cargo build -p meerkat-rpc --bin rkat-rpc --features live-webrtc
export RKAT_RPC="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rkat-rpc"
export OPENAI_API_KEY=sk-...

npm --prefix sdks/typescript install
npm --prefix sdks/typescript run build

cd examples/037-live-webrtc-web
npm install
npm start
```

Open http://127.0.0.1:4173/.

With the server running and Playwright's bundled headless Chromium installed
(`npx playwright install chromium-headless-shell`), the optional headless
connectivity harness can capture fresh local evidence:

```bash
npm run smoke:webrtc
```

It writes `.smoke/webrtc-evidence.json`. That generated evidence is not kept
in the repository because capabilities and provider behavior change over time.
The harness uses synthetic browser media, never a physical microphone or
camera. It still requires a real authenticated live provider. Failures also
write evidence, close the remote channel when acquired, and dispose the browser.
`npm run smoke:webrtc:test` runs the same harness with a real 90-second
Playwright test timeout (the operation itself is bounded to 80 seconds).

## What To Test

- Browser asks for microphone with `echoCancellation`, `noiseSuppression`, and
  `autoGainControl`.
- `live/open` returns a WebRTC bootstrap.
- `live/webrtc/answer` accepts the browser offer and returns an SDP answer.
- WebRTC data channel opens and receives `WireLiveAdapterObservation` JSON.
- Browser audio reaches the provider and produces user transcript observations.
- Assistant audio plays through the WebRTC remote track.
- `Interrupt`, `Refresh`, `Commit`, and `Truncate` call the JSON-RPC live verbs.
- Voice-triggered `mob_create` / `mob_spawn_member` calls create real Meerkat
  mobs and the cockpit polls canonical mob state to show member status.

## Useful Prompts

```text
Remember that the release name is Northstar.
What notes have you saved?
Create a mob that investigates whether the WebRTC live transport is ready to ship.
Ask the skeptic member to focus on rollback risk.
What mobs exist right now?
```

## Notes

JSON-RPC is the canonical signaling/control surface for this MVP. The Node host
launches the SDK client, serves the web assets, executes the two callback tools,
exposes the browser control routes, and polls mob state. Media still terminates
in Meerkat's WebRTC transport, not in Node.

`MEERKAT_LIVE_MODEL` and `MEERKAT_LIVE_WORKER_MODEL` set server-side defaults.
The current page submits its prefilled model fields, so clear or replace those
values in the cockpit if you want different settings.

Start owns a single attempt and is disabled while starting/live. Stop remains
available during startup. Startup has a 45-second aggregate deadline, including
signaling and connection; ICE gathering also has a 10-second bound. Stop/failure
releases capture, peer/data channel, meter and polling, and attempts remote close
with a 5-second bound. Late microphone/open results are still disposed. If the
page disappears during a pending open, the Node host closes the late channel.
Unexpected channel close, failed/closed transport, terminal error, and closed
adapter status all stop the cockpit. `command_rejected`, transient ICE
disconnection, and degraded adapter status are nonterminal.

Spoken and written transcript drafts are correlated by response, item and
content index. Interruptions settle only matching spoken output; truncations
replace it with the supplied heard prefix. Written output remains visible.
Identity-free late spoken events are treated conservatively rather than being
appended to a newer identified answer.
That guard applies after identified as well as anonymous interruptions. An
anonymous turn interruption uses the known active response scope (all its spoken
items), while truncation remains item/content-scoped. Known item identity can
recover an omitted response ID; conflicting associations are not guessed.
Meter cleanup is registered before allocation, and a retired startup cannot
allocate a meter after its microphone promise settles.

## Offline Validation

With the local SDK built and dependencies installed as above:

```bash
npm run check
npm run build
npm test
```

An additional provider-independent smoke can run the built Node host against the
local native RPC binary (it uses an isolated fixture HOME and removes it):

```bash
RKAT_RPC=/absolute/path/to/rkat-rpc node tests/local-host-smoke.cjs
```

It verifies asset/state routes, inactive-channel refusal, runtime reuse and
subprocess teardown without opening a live channel.

The regression suite uses Playwright's bundled headless Chromium (set
`PLAYWRIGHT_CHROMIUM_CHANNEL=chrome` to run it against an installed Google
Chrome instead) with synthetic browser primitives,
fake timers and local HTTP assets, plus fake JSON-RPC executables driven by the
real SDK. It exercises the actual cockpit, shared smoke negotiation, ICE waits,
transcript model and runtime initialization. No physical audio/video or OpenAI
authentication is used. These checks do not establish live provider connectivity,
audible playback, or real microphone echo cancellation.

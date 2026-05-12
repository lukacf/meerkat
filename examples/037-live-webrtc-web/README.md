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
(`mob_create`, `mob_spawn_member`, `mob_list`, etc.) plus one small callback
tool for note capture.
The cockpit shows transcript observations, tool requests, created mobs/member
status, saved notes, WebRTC state, data-channel state, and manual live controls.

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
exists only because browsers cannot speak the SDK's stdio JSON-RPC transport
directly. Media still terminates in Meerkat's WebRTC transport, not in Node.

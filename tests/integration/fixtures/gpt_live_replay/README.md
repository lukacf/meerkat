# gpt-live replay fixtures

Recorded gpt-live-1 provider streams from green Turbo S runs, scrubbed for
commit. Each fixture is the provider side of one live scenario at the public
Live adapter boundary (`meerkat_openai::public_live`): what Meerkat sent and
what the provider answered, in causal order.

## What is recorded

Every live e2e journal (`tests/integration/tests/gpt_live_public_e2e.rs`)
writes `provider-stream.jsonl` beside its `journal.jsonl` (under
`target/e2e-live-audio-artifacts/<scenario>/<uuid>/` locally, or the test's
undeclared-outputs directory on CI). The recorder is test-only code behind the
`test-realtime-fixtures` feature; production builds do not contain it.

One JSON line per adapter crossing:

| `entry.dir`       | Meaning                                                  |
|-------------------|----------------------------------------------------------|
| `create_request`  | `POST /v1/live/sessions` body (session config, offer)   |
| `create_response` | Its response (session identity, answer)                  |
| `client_event`    | A client event Meerkat sent on the sideband              |
| `server_frame`    | A server frame as the provider sent it (lossless `raw`) |
| `receiver_end`    | The sideband ended (`error` set when it failed)          |
| `marker`          | A test-driven step: `play_at:<fixture>`, `queue:<a,b>`,  |
|                   | `disconnect:<graceful\|hard>`, recorded before it runs  |

Each line also carries `seq` (global order), `channel_ordinal` (the journal's
channel) and `elapsed_ms` (informational only; replay never keys on time).

Markers make replay causal without a clock: a frame recorded after a client
event is served only once Meerkat sent that event (matched on event type and
deterministic `event_id`), and a frame recorded after a marker only once the
replaying test reached that step.

A raw recording that lost a line is renamed `provider-stream.jsonl.incomplete`
at journal finish and is never a fixture.

## Re-capture (one command)

```bash
OPENAI_API_KEY=... scripts/gpt-live-recapture-replay-fixture S104   # or S106
```

It builds the live e2e binary, runs the scenario once against gpt-live-1, and
only when the run's journal ends `passed` scrubs the recording into
`<scenario>.provider-stream.jsonl` here. A failed, timed-out or
provider-degraded run writes nothing. Run it on an unloaded host (browser
audio garbles under heavy load), then review the diff and commit.

## Scrubbing and the secret check

`scripts/gpt-live-scrub-provider-stream scrub IN OUT` replaces:

- whole values under credential-like keys (`sdp`, `client_secret`,
  `authorization`, `api_key`, `token`, ...); the SDP carries ICE credentials
  and host candidates and is never needed for replay;
- credential-shaped strings anywhere (OpenAI keys, ephemeral keys, bearer
  tokens, JWTs), SDP ICE lines, email addresses, home-directory paths and IPv4
  addresses;
- voice audio: every audio payload (`session.input_audio.append` reflected
  uplink, `session.output_audio.delta` model speech) becomes
  `<silence-bytes:N>`, N being its decoded length; replay expands it to N
  zero bytes, so audio timing and latency accounting are preserved;
- the exact values of `OPENAI_API_KEY`, `RKAT_OPENAI_API_KEY` and
  `BUILDBUDDY_API_KEY` when set.

It re-checks its output and refuses to write if anything forbidden remains.
`scripts/gpt-live-scrub-provider-stream check DIR` is the committed-fixture
gate: it exits nonzero on any finding and prints only `file:line: rule`, never
the matched text. It runs as the `gpt-live-replay-fixture-scrubbed`
pre-commit hook on every fixture here, and the replay tests apply the same
rules to each fixture they load, so PR CI rejects an unscrubbed fixture.

Scenario content (instructions, transcripts, injected context) is synthetic
test fixture text, so it is kept: replay needs it.

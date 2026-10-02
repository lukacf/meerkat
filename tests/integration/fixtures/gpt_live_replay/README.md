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
pre-commit hook on every fixture here. In PR CI,
`gpt_live_replay::replay_fixtures_are_scrubbed` applies the credential, SDP,
home-path and voice-audio rules to every embedded fixture, so an unscrubbed
fixture fails the lane.

Scenario content (instructions, transcripts, injected context) is synthetic
test fixture text, so it is kept: replay needs it.

## Replay

`tests/integration/tests/gpt_live_replay.rs` (feature `gpt-live-replay`, a
normal non-live target: no provider, no key) replays each fixture against the
same host graph the live scenario drives: RPC server, mob, executor, and the
shared exact-receipt live host with a concurrent bootstrap summary.

```bash
cargo test -p meerkat-integration-tests --features gpt-live-replay --test gpt_live_replay
```

PR CI runs it as the `gpt-live-replay` integration suite
(`scripts/ci-cargo-lanes.mjs`) whenever a package on the public Live path,
this directory or the harness changes.

- **Provider:** `support/gpt_live_replay.rs`'s `Cassette` serves the fixture
  as a local public Live API, reached through
  `ExperimentalGptLiveOpenAuthority::with_test_base_url`. Each channel's tape
  is walked in recorded order:
  - a server frame is sent;
  - a recorded client event is awaited until Meerkat sends one with the same
    type and `event_id` (early arrival counts; payloads are not compared);
  - a marker is awaited until the test steps it with `Cassette::release`;
  - the receiver end closes the sideband.

  Nothing waits on a clock.
- **LLMs:** the executor's and the delegation worker's turns are a scripted
  client keyed by purpose:
  - the conversational ordinal;
  - the delegated job (its request carries
    `LIVE_DELEGATION_SPEECH_TRANSCRIPT_NOTE`);
  - the source member's reply to a merged post-close result.

  Answers whose length shapes the client events (an appended reply is split
  into 500-byte fragments, and the fragment index is in the `event_id`) are
  taken from the fixture itself. The summary is a fixed stub.
- **Ordering the product leaves open:** the test fixes it with typed gates,
  matching what the recorded run did. For S104, the job's worker turn is
  released after channel 1 is closed, and the merge reply after channel 2 is
  connected. Each browser step is taken only once the host reached the state
  the live run had reached; for example, the disconnect waits until the job's
  worker has started.
- **Assertions:**
  - the replay records its own provider stream through the same journal hook,
    and its client events per channel must equal the fixture's;
  - each open's seed shape must match;
  - the scenario's own contract holds (S104: the merged job's reply reaches the
    reopened channel as runtime work carrying the result token).

A divergence fails with where the tape is parked (the marker or client event
it waits for). `REPLAY_DUMP_REQUESTS=1` prints every scripted LLM request with
its classified purpose.

# mock_realtime_ws

Deterministic, in-process mock for the OpenAI Realtime session protocol,
used by `e2e-fast` tests that need realtime lifecycle coverage without live
OpenAI credentials. Landed in W1-C (roadmap issue #264).

## Why

Axis-crossing bugs like the s71 turn-8 regression (peer-response-triggered
turn in turn-driven realtime mode) went unnoticed because every realtime
test path required live API credentials and ran only in `e2e-live` / `e2e-smoke`.
The deterministic `e2e-fast` lane had no realtime coverage at all.

This harness is the other half of the W1-C composite-path coverage matrix
(`tests/integration/src/coverage_matrix.rs`): the matrix tells you which
cells need coverage, this mock is what makes those cells testable.

## Where it sits

It implements `meerkat_llm_core::realtime_session::{RealtimeSession,
RealtimeSessionFactory}` directly. The `RealtimeMockServer` is a test-only
factory that returns `RealtimeMockSession`s driven by pre-scripted scenarios.

Note on the "WS" in the name: Meerkat's provider contract for realtime is
the `RealtimeSession` trait, not a raw websocket. In production, `oai-rt-rs`
handles the websocket framing behind that trait. At the trait boundary,
tests get deterministic replay without a TCP listener. If a future test
needs raw `response.output_text.delta` / `response.done` JSON frames on a
socket, extend this module with a `tokio_tungstenite`-based listener that
serializes the same `ScriptedEvent` vocabulary.

## How to use

```rust
use mock_realtime_ws::{RealtimeMockServer, ScriptedEvent, ScriptedScenario};

let server = RealtimeMockServer::new();
server
    .enqueue(
        ScriptedScenario::new()
            .with_event(ScriptedEvent::TurnStarted)
            .with_event(ScriptedEvent::OutputTextDelta("hello".into()))
            .with_event(ScriptedEvent::TurnCompleted {
                stop_reason: StopReason::EndTurn,
            })
            .with_event(ScriptedEvent::End),
    )
    .await;

let factory = server.factory();
// hand `factory` to the code under test wherever a
// `RealtimeSessionFactory` is required.
```

Every `open_session` consumes one queued scenario in FIFO order. Missing
scenarios produce a loud `LlmError::InvalidRequest` instead of a hang.

After the scenario runs, inspect what was sent provider-wards:

```rust
let recording = server.recording_for(0).await.unwrap();
let recording = recording.lock().await;
assert_eq!(recording.inputs.len(), 1);
assert_eq!(recording.commits, 1);
assert!(recording.closed);
```

## Scope discipline

The mock is intentionally thin. It covers:

- `open_session` flow (capabilities, turning mode)
- `send_input`, `commit_turn`, `interrupt`, `truncate_assistant_output`
- `submit_tool_result`, `submit_tool_error`
- `next_event` scripted playback
- `close`

It does NOT cover:

- External session attachment (`attach_external_session` fails loudly —
  production code paths that rely on it should exercise the live lane).
- Adaptive scripting based on provider side-effects (if your test needs
  provider behavior that depends on what the code under test sent, write a
  custom `RealtimeSession` for that one test rather than expanding the mock).
- Real WS framing (see note above).

Keep it fast, keep it obvious, keep it test-only.

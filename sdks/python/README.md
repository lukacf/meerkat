# Meerkat Python SDK

Python SDK for the [Meerkat](https://github.com/lukacf/meerkat) runtime.

- **Contract version:** `0.8.40`
- **Python:** `>=3.10`
- **Package:** `meerkat-sdk`

The SDK is session-first and runtime-backed: it spawns `rkat-rpc` and exposes typed Python handles (`Session`, `DeferredSession`, `Mob`) over the same canonical JSON-RPC surface used by CLI/REST/MCP.

## Install

```bash
pip install meerkat-sdk
```

For local development:

```bash
pip install -e "sdks/python[dev]"
```

## Quick start

The selected model needs a compatible credential route: an already provisioned
auth binding or the provider's environment key. This example selects Claude.
For a clean API-key setup, export your Anthropic key in the shell that starts
Python; the `rkat-rpc` child inherits it:

```bash
export ANTHROPIC_API_KEY="<your-anthropic-api-key>"
```

Skip this export if you already use a compatible auth binding; pass it with
`auth_binding` when it is not the configured default. A different provider's
key is not a substitute for the selected model's credentials.

```python
import asyncio
from meerkat import MeerkatClient

async def main() -> None:
    async with MeerkatClient() as client:
        session = await client.create_session(
            "What is the capital of France?",
            model="claude-sonnet-4-6",
        )
        print(session.text)

        result = await session.turn("And Germany?")
        print(result.text)

        await session.archive()

asyncio.run(main())
```

## Core API

### Session creation

`create_session`, `create_session_streaming`, and `create_deferred_session` support:

- host context and identity: `injected_context`, `auth_binding`, plus
  `transient_turn_context` for immediate create/stream only
- model/provider controls: `model`, `provider`, `max_tokens`, `system_prompt`, `provider_params`
- structured output: `output_schema`, `structured_output_retries`
- runtime/tool toggles: `enable_builtins`, `enable_shell`, `enable_memory`, `enable_schedule`, `enable_workgraph`, `enable_mob`, `enable_web_search`
- comms/runtime metadata: `keep_alive`, `comms_name`, `peer_meta`, `budget_limits`
- skills: `preload_skills`, `skill_refs`
- session metadata: `labels`
- additional session config: `additional_instructions`, `app_context`, `shell_env`, `external_tools`

Deferred creation stores injected context for its eventual first turn but does
not accept transient turn context. Pass transient facts to
`DeferredSession.start_turn(...)`.

### Session queries

- `await client.list_sessions(...) -> list[SessionSummary]`
- `await client.read_session(session_id) -> SessionDetails`
- `await client.read_session_history(session_id, offset=0, limit=None) -> SessionHistory`

`SessionSummary` is returned by `session/list` and includes `total_tokens`.
`SessionDetails` is returned by `session/read` and includes `model`, `provider`, and `last_assistant_text`.
Both use integer unix timestamps for `created_at` and `updated_at`.

### Session runtime inputs

- `await client.inject_context(session_id, text, source=..., idempotency_key=...)`
- `await session.inject_context(...)`
- `await client.send_external_event(session_id, event_type, payload, blocks=None)`
- `await session.send_external_event(event_type, payload, blocks=None)`

### Turns and streaming

`Session.turn(...)` and `Session.stream(...)` now expose full turn overrides:

- `injected_context`, `transient_turn_context`
- `skill_refs`, `turn_tool_overlay`
- `additional_instructions`
- `keep_alive`, `model`, `provider`, `self_hosted_server_id`, `max_tokens`, `system_prompt`
- `output_schema`, `structured_output_retries`, `provider_params`

`DeferredSession.start_turn(...)` supports the same override set.

### Config APIs return config write results

`get_config` returns a `ConfigEnvelope`:

```python
{
  "config": {...},
  "generation": 12,
  "realm_id": "...",
  "instance_id": "...",
  "backend": "...",
  "resolved_paths": {...} | None,
}
```

`set_config` and `patch_config` return the same envelope fields plus optional
`live_propagation` when a write fans out to live channels.

Example:

```python
config_envelope = await client.get_config()
updated = await client.patch_config(
    {"max_tokens": 2048},
    expected_generation=config_envelope["generation"],
)
```

## Additional surfaces

### Models catalog

- `await client.get_models_catalog() -> ModelsCatalogResponse`

### Schedules

- `await client.create_schedule(request)`
- `await client.get_schedule(schedule_id)`
- `await client.list_schedules(labels=None, limit=None, offset=None)`
- `await client.update_schedule(request)`
- `await client.pause_schedule(schedule_id)`
- `await client.resume_schedule(schedule_id)`
- `await client.delete_schedule(schedule_id)`
- `await client.list_schedule_occurrences(schedule_id, include_terminal=True|False)`
- `await client.list_schedule_tools()`
- `await client.call_schedule_tool({"name": "...", "arguments": {...}})`

### WorkGraph

WorkGraph is exposed as read-only observability from the SDK. Agents mutate
work through their WorkGraph tools; SDK callers inspect the current graph.

- `await client.get_workgraph_item(item_id, realm_id=None, namespace=None)`
- `await client.list_workgraph_items(filter=None)`
- `await client.list_ready_workgraph_items(filter=None)`
- `await client.get_workgraph_snapshot(filter=None)`
- `await client.list_workgraph_events(filter=None)`
- `await client.get_workgraph_goal_status(GoalStatusRequest(...))`
- `await client.list_workgraph_attention(AttentionListRequest(...))`

### Jobs, approvals, artifacts, and projected event replay

- jobs: `jobs_get/list/cancel/progress/result/artifacts/retry/health/subscribe/unsubscribe`
- monitors and MobKit workers: `monitors_start`, `mobkit_job_*`
- approvals: `request_approval`, `list_approvals`, `get_approval`, `decide_approval`
- artifacts: `list_artifacts`, `get_artifact`, `download_artifact`
- projected event recovery: `latest_event_cursor`, `list_events_since`,
  `event_snapshot` when host event projection is enabled

Approval calls maintain audit records only; they do not automatically gate,
authorize, or execute an action. They persist when the RPC bundle exposes a
store path and otherwise remain process-local.

Explicitly ephemeral memory realms do not expose event cursor or snapshot
replay APIs.

Every public Python RPC wrapper binds its request and result transport boundary
to generated RPC-schema contracts. Generated request/result classes for the
families above currently live in `meerkat.generated.types`; they are not
re-exported at the package root.

Auth profiles and credentials are realm-scoped. Provisioning yields an
`auth_binding` for a session or mob member spec, so the spec does not carry the
provider secret. Remote hosts resolve bindings only inside their authorized
realm.

### Transcript editing and reconciliation

- `input_state`, `export_session_atif`
- transcript revision read/list, fork-at/fork-replace, rewrite, system-prompt
  update, and revision restore

### Mob runtime

- `await client.read_mob_events(mob_id, after_cursor=0, limit=100)`
- `await mob.read_events(after_cursor=0, limit=100)`
- `await mob.spawn_many(specs) -> MobSpawnManyResult`
- `await mob.wait_for_kickoff_complete(...)`
- helper methods use canonical `role_name`
- trusted multi-host controls cover host bind/revoke, route installs,
  scope grants, hard cancel, member history, and remote member live channels

Multi-host mutations are trusted host APIs. They are not agent-callable, and
the agent spawn wire cannot declare Rust's one-shot `resume_from_role` role
migration authority.

### Live WebSocket and WebRTC

`LiveChannel` wraps the session-bound live lifecycle. `live_open` returns a
discriminated `websocket` or `webrtc` bootstrap. Only the WebSocket variant has
`url`; WebRTC callers create an SDP offer and call
`live_webrtc_answer(channel_id, token, offer_sdp)`.

WebRTC requires `connect(live_webrtc=True)` and an `rkat-rpc` binary compiled
with its non-default `live-webrtc` Cargo feature. The 0.8.24 auto-downloaded
release binary omits that feature. Build a custom binary with
`./scripts/repo-cargo build -p meerkat-rpc --features live-webrtc`, and add an
audio track or the `meerkat.live` data channel before the browser creates its
offer. Set the local description, wait for ICE gathering to complete, and send
`peer.localDescription.sdp`; there is no candidate-trickle RPC. Install the
returned `answer_sdp` as the browser peer's remote description before live
input. WebSocket uses `connect(live_ws=True)` and does not need that Cargo
feature.

### Realm profile CRUD

- `await client.create_mob_profile(name, profile)`
- `await client.get_mob_profile(name)` returns `StoredMobProfile | None`
- `await client.list_mob_profiles()`
- `await client.update_mob_profile(name, profile, expected_revision=...)`
- `await client.delete_mob_profile(name, expected_revision=...)`

## Streaming

`session.stream(...)` and `client.create_session_streaming(...)` return `EventStream`, an async context manager yielding typed event dataclasses (`TextDelta`, `TurnCompleted`, `ToolExecutionCompleted`, etc.).

```python
from meerkat import TextDelta

async with session.stream("Explain this in detail.") as events:
    async for event in events:
        match event:
            case TextDelta(delta=chunk):
                print(chunk, end="", flush=True)
    result = events.result
```

`TurnCompleted.usage` is optional; absent means unmeasured, not zero.
`RunStarted.input` preserves the typed `RunInput`: content (text or blocks) or
a `pending_tool_results` continuation without a fabricated prompt.
`RunFailed` accepts the current `error_report` and derives its display fields
from that report.

Inventory-known events without a handwritten parser class arrive as
`UnknownEvent`, preserving `event.type` and the complete wire payload in
`event.data`. This includes `server_tool_content`, all four `model_fallback_*`
events, `transcript_rewrite_audit_receipt_committed`, `boundary_append_applied`,
and `boundary_appends_discarded`. A present type outside the generated inventory
raises `MeerkatError` with code `UNKNOWN_EVENT_TYPE`.

A missing type or malformed payload for a handwritten parser class arrives as
`UnknownEvent(type="malformed_event")`, with the original payload in
`event.data`. The `Retrying` and `HookFailed` parsers still expect legacy flat
fields; current payloads carry `retry` and `reason`, respectively, so they take
this malformed path. See the [event compatibility
reference](../../docs/sdks/python/reference.mdx#event-compatibility-note) for details.

## Run tests

```bash
pip install -e "sdks/python[dev]"

pytest sdks/python/tests/test_types.py -v
pytest sdks/python/tests/test_audit_parity.py -v
pytest sdks/python/tests/test_e2e.py -v
pytest sdks/python/tests/test_e2e_smoke.py -v
```

# Meerkat Python SDK

Python SDK for the [Meerkat](https://github.com/lukacf/meerkat) runtime.

- **Contract version:** `0.6.0`
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

```python
import asyncio
from meerkat import MeerkatClient

async def main() -> None:
    async with MeerkatClient() as client:
        session = await client.create_session("What is the capital of France?")
        print(session.text)

        result = await session.turn("And Germany?")
        print(result.text)

        await session.archive()

asyncio.run(main())
```

## Core API

### Session creation

`create_session`, `create_session_streaming`, and `create_deferred_session` support:

- model/provider controls: `model`, `provider`, `max_tokens`, `system_prompt`, `provider_params`
- structured output: `output_schema`, `structured_output_retries`
- runtime/tool toggles: `enable_builtins`, `enable_shell`, `enable_memory`, `enable_mob`
- comms/runtime metadata: `keep_alive`, `comms_name`, `peer_meta`, `budget_limits`
- skills: `preload_skills`, `skill_refs`, `skill_references`
- session metadata: `labels`
- additional session config: `additional_instructions`, `app_context`, `shell_env`, `external_tools`

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
- `await client.send_external_event(session_id, payload, source=...)`
- `await session.send_external_event(...)`

### Turns and streaming

`Session.turn(...)` and `Session.stream(...)` now expose full turn overrides:

- `skill_refs`, `skill_references`, `flow_tool_overlay`
- `additional_instructions`
- `keep_alive`, `model`, `provider`, `max_tokens`, `system_prompt`
- `output_schema`, `structured_output_retries`, `provider_params`

`DeferredSession.start_turn(...)` supports the same override set.

### Config APIs return envelopes

`get_config`, `set_config`, and `patch_config` return a `ConfigEnvelope`:

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

Example:

```python
config_envelope = await client.get_config()
updated = await client.patch_config(
    {"agent": {"max_tokens": 2048}},
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

### Mob runtime

- `await client.read_mob_events(mob_id, after_cursor=0, limit=100)`
- `await mob.read_events(after_cursor=0, limit=100)`
- `await mob.spawn_many(specs)`
- `await mob.wait_for_kickoff_complete(...)`
- helper methods use canonical `role_name` (legacy `profile_name` alias is still accepted)

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

## Run tests

```bash
pip install -e "sdks/python[dev]"

pytest sdks/python/tests/test_types.py -v
pytest sdks/python/tests/test_audit_parity.py -v
pytest sdks/python/tests/test_e2e.py -v
pytest sdks/python/tests/test_e2e_smoke.py -v
```

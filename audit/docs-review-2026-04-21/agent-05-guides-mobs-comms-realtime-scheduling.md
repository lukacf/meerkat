# Agent 05 Findings

## Scope reviewed

- Guides: comms, mobs, mobpack, realtime, scheduling
- Examples: comms, mobs, mobpack, wasm
- Validation sources: current RPC handlers, schedule types/tool schemas, web runtime exports, and Python/TypeScript SDK surfaces
- No factual drift found in the mobpack packaging/trust-policy docs. `docs/guides/mobpack.mdx` and `docs/examples/mobpack.mdx` matched the current CLI nouns and flow closely enough during this pass.

## Files reviewed

- `docs/guides/comms.mdx`
- `docs/guides/mobs.mdx`
- `docs/guides/mobpack.mdx`
- `docs/guides/realtime.mdx`
- `docs/guides/scheduling.mdx`
- `docs/examples/comms.mdx`
- `docs/examples/mobs.mdx`
- `docs/examples/mobpack.mdx`
- `docs/examples/wasm.mdx`
- `docs/api/rpc.mdx`
- `docs/api/mcp.mdx`
- `meerkat-rpc/src/handlers/comms.rs`
- `meerkat-rpc/src/handlers/mob.rs`
- `meerkat-rpc/src/handlers/realtime.rs`
- `meerkat-rpc/src/router.rs`
- `meerkat-core/src/comms.rs`
- `meerkat-core/src/model_profile/capabilities/openai.rs`
- `meerkat-schedule/src/types.rs`
- `meerkat-schedule/src/tools.rs`
- `meerkat-web-runtime/src/lib.rs`
- `sdks/python/meerkat/client.py`
- `sdks/python/meerkat/session.py`
- `sdks/python/meerkat/mob.py`
- `sdks/typescript/src/client.ts`
- `sdks/typescript/src/session.ts`

## Findings

1. `docs/guides/mobs.mdx` still documents stale mob tool names and blurs the agent-side vs host-side split. The table at `docs/guides/mobs.mdx:178-200` lists tools like `spawn_member`, `spawn_many_members`, `wire_members`, and `unwire_members`, then says those tools are available on every surface. Current host surfaces expose typed `mob/*` methods such as `mob/spawn`, `mob/spawn_many`, `mob/wire`, `mob/unwire`, `mob/flow_run`, and `mob/flow_status` instead (`docs/api/rpc.mdx:82-122`, `meerkat-rpc/src/handlers/mob.rs:203-420`, `meerkat-rpc/src/handlers/mob.rs:534-904`). The public MCP surface likewise exposes `meerkat_mob_*` control-plane tools, not raw agent-side `mob_*` tools (`docs/api/mcp.mdx:71-76`). As written, the guide teaches a surface contract that no longer matches the canonical RPC/MCP/SDK API.

2. `docs/examples/mobs.mdx` keeps using agent-tool workflows in tabs that are labeled like direct surface examples, and it has a visible duplicated block. In the “Agent-side mob tools reference” section the agent tool names are at least current, but the later JSON-RPC/REST/MCP/Python/TypeScript tabs for “Spawn members” and “Wire and unwire peers” tell readers to call `mob_spawn_member` / `mob_wire` tool payloads instead of the current host APIs (`docs/examples/mobs.mdx:228-289`). The current direct surfaces are `mob/spawn` / `mob/wire` on RPC (`docs/api/rpc.mdx:86-101`, `meerkat-rpc/src/handlers/mob.rs:268-326`, `meerkat-rpc/src/handlers/mob.rs:542-590`) and first-class `Mob.spawn()` / `Mob.wire()` on the SDKs (`sdks/python/meerkat/mob.py:220-242`, `sdks/python/meerkat/mob.py:338-342`). There is also an accidental duplicated “Spawn members” block in `docs/examples/mobs.mdx:218-260`, which makes the stale examples harder to trust.

3. `docs/examples/comms.mdx` mixes the old external-event shape with the new typed comms API, and the SDK snippets are no longer valid. The RPC/REST examples in “Send a message” and “External event injection” use `payload` plus `source` without a `kind` discriminator (`docs/examples/comms.mdx:92-147`, `docs/examples/comms.mdx:198-259`), but the current `comms/send` handler expects the flat `CommsCommandRequest` wire shape with `kind: "input" | "peer_message" | "peer_request" | "peer_response"` (`meerkat-rpc/src/handlers/comms.rs:97-180`, `meerkat-core/src/comms.rs:71-123`). The Python and TypeScript SDK snippets are also stale: `session.send(payload=..., source=...)` / `session.send({ payload, source })` no longer match the SDK signatures, which now require a typed comms command (`sdks/python/meerkat/session.py:314-322`, `sdks/python/meerkat/client.py:2065-2108`, `sdks/typescript/src/session.ts:154-160`, `sdks/typescript/src/client.ts:1803-1809`). This should be split into two clearly different flows: `session/external_event` or `sendExternalEvent(...)` for durable external events, and `comms/send` or `session.send({ kind: ... })` for typed comms.

4. `docs/guides/scheduling.mdx` is describing an older scheduler contract and will mislead anyone trying to use the current API. Specific drift:
- It uses `schedule_create` via `tool/call` in the RPC example (`docs/guides/scheduling.mdx:59-80`), but the canonical host methods are `schedule/create`, `schedule/list`, `schedule/update`, `schedule/occurrences`, etc. (`docs/api/rpc.mdx:123-132`).
- It describes `type: "cron"` plus `values` / `range` field objects (`docs/guides/scheduling.mdx:85-103`), but the current trigger model is `once`, `interval`, or `calendar`, with calendar fields encoded as `{ "kind": "any" | "values" }` (`meerkat-schedule/src/types.rs:147-205`, `meerkat-schedule/src/tools.rs:73-90`).
- It shows an interval trigger with only `every_seconds` (`docs/guides/scheduling.mdx:108-113`), but `IntervalTriggerSpec` now requires `start_at_utc` and optionally `end_at_utc` (`meerkat-schedule/src/types.rs:156-163`).
- It shows simplified target shapes like `{ "type": "session", "model": ..., "prompt": ... }` and `{ "type": "mob", "action": "create_and_run", ... }` (`docs/guides/scheduling.mdx:117-141`), but the real wire model is `target_kind: "session" | "mob"` with typed variants such as `exact_session`, `resumable_session`, `materialize_on_demand_session`, `member`, `flow`, `spawn_helper`, and `fork_helper` (`meerkat-schedule/src/types.rs:280-326`, `meerkat-schedule/src/types.rs:496-527`).
- The policy names and lifecycle states are also outdated. Current defaults are `misfire_policy = {"type":"skip"}`, `overlap_policy = "skip_if_running"`, `missing_target_policy = "mark_misfired"` (`meerkat-schedule/src/tools.rs:109-125`, `meerkat-schedule/src/types.rs:263-278`), while schedule phase is now only `active | paused | deleted` and occurrence phase includes `awaiting_completion`, `misfired`, and `delivery_failed` (`meerkat-schedule/src/types.rs:97-145`).

5. `docs/guides/realtime.mdx` is missing an important lifecycle caveat: reaching `binding_ready` is not sufficient by itself to guarantee `realtime/open_info` works on a given host. The guide currently tells readers to poll `runtime/realtime_attachment_status` and then call `realtime/open_info` (`docs/guides/realtime.mdx:167-180`), but the handler explicitly returns capability-unavailable when the realtime WebSocket host is not wired in (`meerkat-rpc/src/handlers/realtime.rs:219-224`, routed from `meerkat-rpc/src/router.rs:1179-1211`). The guide should mention that bootstrap methods depend on a host that actually exposes `RealtimeWsHost`, otherwise readers will treat attachment readiness as equivalent to transport availability. Separately, the model guidance is slightly stale: the catalog now treats `gpt-realtime-1.5` as the canonical realtime model and `gpt-realtime` as a supported legacy alias (`meerkat-core/src/model_profile/capabilities/openai.rs:109-163`), so the guide should either prefer `gpt-realtime-1.5` in examples or call out the aliasing explicitly.

6. `docs/examples/wasm.mdx` has a wrong `wire_cross_mob` call signature. The example passes three arguments at `docs/examples/wasm.mdx:113-115`, but the export takes four: `wire_cross_mob(mob_a, agent_a, mob_b, agent_b)` (`meerkat-web-runtime/src/lib.rs:1932-1937`). Anyone copying the snippet will fail before they even get to the comms behavior the example is trying to demonstrate.

## Suggested follow-ups

1. Rewrite the mob docs so each page explicitly separates:
- agent-side `mob_*` tools inside a running session
- typed host APIs: `mob/*` over RPC/SDKs and `meerkat_mob_*` over public MCP

2. Rebuild the scheduling guide from the current schedule schema instead of prose memory. `meerkat-schedule/src/tools.rs` plus `meerkat-schedule/src/types.rs` is the best source of truth for trigger, target, and policy shapes.

3. Split comms examples into two sections with different contracts:
- typed comms commands via `comms/send` / `session.send({ kind: ... })`
- durable external events via `session/external_event` or `sendExternalEvent(...)`

4. Add a realtime caveat note near the bootstrap flow stating that `realtime/open_info` requires a host with the realtime WebSocket service enabled, and refresh examples to prefer `gpt-realtime-1.5` while mentioning `gpt-realtime` as the compatibility alias.

5. Fix the WASM cross-mob snippet and remove the duplicated “Spawn members” block in `docs/examples/mobs.mdx` during the same cleanup pass.
